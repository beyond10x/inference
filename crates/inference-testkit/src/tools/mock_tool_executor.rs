//! The scripted mock of the executor contract of `inference-tools`.
//!
//! A [`MockToolExecutor`] is constructed with a table of tools, each with a [`Script`]: the result
//! it answers, the typed failure it answers, or an overrun, which answers no result before the
//! deadline. It records every invocation, with the call's identity, before it answers, so a test
//! reads how often and with what each call reached it; a call for a tool the table lacks is
//! recorded and fails with an `execution_error` naming the tool.

use std::collections::HashMap;
use std::future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Poll;
use std::time::Instant;

use inference_core::id::GenerationId;
use inference_tools::{
    CancellationSignal, ExecutionFuture, Json, ToolCall, ToolCallId, ToolExecutor, ToolFailure,
    ToolFailureKind, ToolId,
};

/// What the mock answers for one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Script {
    /// Answers this result at once.
    Result(Json),
    /// Answers this failure at once.
    Failure(ToolFailure),
    /// Answers nothing before the deadline: a `deadline_exceeded` failure once it passed, or a
    /// `cancelled` one once the call was cancelled. It wakes no task itself, so its caller polls
    /// it again by the deadline.
    Overrun,
}

/// One invocation the mock received: the call it executed, by identity, with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub call_id: ToolCallId,
    pub tool_id: ToolId,
    pub generation_id: GenerationId,
    pub arguments: Json,
}

/// The scripted mock executor; see the module documentation. Clones share one table and one log.
#[derive(Debug, Clone, Default)]
pub struct MockToolExecutor {
    table: Arc<HashMap<ToolId, Script>>,
    invocations: Arc<Mutex<Vec<Invocation>>>,
}

impl MockToolExecutor {
    /// A mock answering each tool of `table` with its script.
    #[must_use]
    pub fn new(table: impl IntoIterator<Item = (ToolId, Script)>) -> Self {
        Self {
            table: Arc::new(table.into_iter().collect()),
            invocations: Arc::default(),
        }
    }

    fn log(&self) -> MutexGuard<'_, Vec<Invocation>> {
        self.invocations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Every invocation the mock received, in the order it received them.
    #[must_use]
    pub fn invocations(&self) -> Vec<Invocation> {
        self.log().clone()
    }
}

impl ToolExecutor for MockToolExecutor {
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        deadline: Instant,
        cancellation: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        self.log().push(Invocation {
            call_id: call.call_id(),
            tool_id: call.tool_id(),
            generation_id: call.generation_id(),
            arguments: call.arguments().clone(),
        });
        match self.table.get(&call.tool_id()) {
            Some(Script::Result(result)) => Box::pin(future::ready(Ok(result.clone()))),
            Some(Script::Failure(failure)) => Box::pin(future::ready(Err(failure.clone()))),
            Some(Script::Overrun) => Box::pin(future::poll_fn(move |_| {
                if cancellation.is_cancelled() {
                    Poll::Ready(Err(ToolFailure {
                        kind: ToolFailureKind::Cancelled,
                        detail: "the call was cancelled before it answered".to_owned(),
                    }))
                } else if Instant::now() >= deadline {
                    Poll::Ready(Err(ToolFailure {
                        kind: ToolFailureKind::DeadlineExceeded,
                        detail: "the call overran its deadline".to_owned(),
                    }))
                } else {
                    Poll::Pending
                }
            })),
            None => {
                let detail = format!("the mock holds no script for tool {}", call.tool_id());
                Box::pin(future::ready(Err(ToolFailure {
                    kind: ToolFailureKind::ExecutionError,
                    detail,
                })))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};
    use std::time::{Duration, Instant};

    use inference_core::generation::{Generation, StartGeneration};
    use inference_core::id::{NodeId, RunId, StreamId};
    use inference_text::{OpenTurn, Span, Turn};
    use inference_tools::{
        CancellationSignal, DeclareTool, SideEffectClass, Tool, ToolCall, ToolExecutor,
        ToolFailure, ToolFailureKind,
    };
    use serde_json::json;

    use super::{MockToolExecutor, Script};

    fn tool(name: &str) -> Tool {
        Tool::declare(DeclareTool {
            name: name.to_owned(),
            schema: json!({"type": "object"}),
            schema_revision: "1".to_owned(),
            side_effect: SideEffectClass::ReadOnly,
            requires_confirmation: false,
        })
        .0
    }

    fn proposed(tool: &Tool) -> ToolCall {
        let (generation, _) = Generation::start(StartGeneration {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
        });
        let (turn, _) = Turn::open(OpenTurn {
            run_id: RunId::generate(),
        });
        let (span, _) = Span::emit(
            &turn,
            turn.run_id(),
            StreamId::generate(),
            1,
            "x".to_owned(),
        )
        .expect("an Open turn admits a span");
        let (span, _) = span.commit().expect("a Hypothesis commits");
        ToolCall::propose(tool, &generation, &span, json!({"n": 1}))
            .expect("a proposal on a committed span")
            .0
    }

    fn poll<F: Future>(future: F) -> Poll<F::Output> {
        pin!(future).poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn each_script_answers_as_written_and_every_invocation_is_logged_with_its_call() {
        let (answers, fails, overruns, unknown) =
            (tool("answers"), tool("fails"), tool("overruns"), tool("x"));
        let failure = ToolFailure {
            kind: ToolFailureKind::ExecutionError,
            detail: "down".to_owned(),
        };
        let mock = MockToolExecutor::new([
            (answers.tool_id(), Script::Result(json!({"ok": true}))),
            (fails.tool_id(), Script::Failure(failure.clone())),
            (overruns.tool_id(), Script::Overrun),
        ]);
        let signal = CancellationSignal::new();
        let later = Instant::now() + Duration::from_secs(60);
        let calls = [
            proposed(&answers),
            proposed(&fails),
            proposed(&overruns),
            proposed(&unknown),
        ];
        assert_eq!(
            poll(mock.execute(&calls[0], later, &signal)),
            Poll::Ready(Ok(json!({"ok": true})))
        );
        assert_eq!(
            poll(mock.execute(&calls[1], later, &signal)),
            Poll::Ready(Err(failure))
        );
        assert!(poll(mock.execute(&calls[2], later, &signal)).is_pending());
        let past = Instant::now();
        assert!(matches!(
            poll(mock.execute(&calls[2], past, &signal)),
            Poll::Ready(Err(ToolFailure {
                kind: ToolFailureKind::DeadlineExceeded,
                ..
            }))
        ));
        assert!(matches!(
            poll(mock.execute(&calls[3], later, &signal)),
            Poll::Ready(Err(ToolFailure {
                kind: ToolFailureKind::ExecutionError,
                ..
            }))
        ));
        signal.cancel();
        assert!(matches!(
            poll(mock.execute(&calls[2], later, &signal)),
            Poll::Ready(Err(ToolFailure {
                kind: ToolFailureKind::Cancelled,
                ..
            }))
        ));
        let logged: Vec<_> = mock
            .invocations()
            .iter()
            .map(|invocation| invocation.call_id)
            .collect();
        let expected = [0, 1, 2, 2, 3, 2].map(|index| calls[index].call_id());
        assert_eq!(logged, expected);
        assert_eq!(
            mock.invocations()[0].generation_id,
            calls[0].generation_id()
        );
        assert_eq!(mock.invocations()[0].arguments, json!({"n": 1}));
    }
}
