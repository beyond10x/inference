//! The scripted mock of the executor contract of `inference-tools`.
//!
//! A [`MockToolExecutor`] is constructed with a table of tools, each with a [`Script`]: the result
//! it answers, the typed failure it answers to every attempt, a failure it answers to the first
//! attempts of a call before it answers a result, an overrun, which answers no result before the
//! deadline, or a hold, which answers nothing until the test releases the call
//! ([`MockToolExecutor::release`]) and then finishes or fails it as scripted. It records every
//! invocation, every attempt among them, with the call's identity, before it answers, so a test
//! reads how often and with what each call reached it; a call for a tool the table lacks is
//! recorded and fails with an `execution_error` naming the tool.
//!
//! The mock's log ([`MockToolExecutor::entries`]) holds, besides the invocations, an entry
//! `cancelled` carrying a held call's call_id, written once when the cancellation signal of the
//! contract fires on the call while it is held, and the answer a held call gave once it was
//! released, so that a test reads that the signal reached a running call from the log rather than
//! from the call's final state. A held call keeps executing after the signal: like a tool that
//! cannot be stopped, it answers on release whatever the signal says (`docs/design/inference.md`
//! 641).

use std::collections::HashMap;
use std::future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Poll, Waker};
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
    /// Answers this failure at once, to every attempt: it fails permanently.
    Failure(ToolFailure),
    /// Answers `failure` at once to the first `failures` attempts of each call, told apart by its
    /// call_id, and `result` at once to every later one.
    FailThenSucceed {
        failures: usize,
        failure: ToolFailure,
        result: Json,
    },
    /// Answers nothing before the deadline: a `deadline_exceeded` failure once it passed, or a
    /// `cancelled` one once the call was cancelled. It wakes no task itself, so its caller polls
    /// it again by the deadline.
    Overrun,
    /// Holds each call until the test releases it, then answers this: a result, which finishes
    /// the call, or a failure, which fails it. The cancellation signal does not end the hold; it
    /// is logged ([`Entry::Cancelled`]). A call is held once: an attempt after its release
    /// answers at once.
    Hold(Result<Json, ToolFailure>),
}

/// One entry of the mock's log, in the order the mock wrote them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// The mock was invoked with a call.
    Invoked(Invocation),
    /// `cancelled`: the cancellation signal fired on the held call carrying this call_id.
    Cancelled(ToolCallId),
    /// The held call carrying this call_id was released and answered this.
    Released(ToolCallId, Result<Json, ToolFailure>),
}

/// One invocation the mock received: the call it executed, by identity, with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub call_id: ToolCallId,
    pub tool_id: ToolId,
    pub generation_id: GenerationId,
    pub arguments: Json,
}

/// A call the mock holds.
#[derive(Debug, Default)]
struct Held {
    released: bool,
    /// Whether its `cancelled` entry was written.
    cancelled: bool,
    /// The task waiting on the call, woken on release.
    waker: Option<Waker>,
}

/// The mock's log and the calls it holds.
#[derive(Debug, Default)]
struct Log {
    entries: Vec<Entry>,
    held: HashMap<ToolCallId, Held>,
}

/// The scripted mock executor; see the module documentation. Clones share one table and one log.
#[derive(Debug, Clone, Default)]
pub struct MockToolExecutor {
    table: Arc<HashMap<ToolId, Script>>,
    log: Arc<Mutex<Log>>,
}

impl MockToolExecutor {
    /// A mock answering each tool of `table` with its script.
    #[must_use]
    pub fn new(table: impl IntoIterator<Item = (ToolId, Script)>) -> Self {
        Self {
            table: Arc::new(table.into_iter().collect()),
            log: Arc::default(),
        }
    }

    fn log(&self) -> MutexGuard<'_, Log> {
        self.log.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every invocation the mock received, in the order it received them.
    #[must_use]
    pub fn invocations(&self) -> Vec<Invocation> {
        self.log()
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Invoked(invocation) => Some(invocation.clone()),
                Entry::Cancelled(_) | Entry::Released(..) => None,
            })
            .collect()
    }

    /// Every entry of the mock's log, in the order the mock wrote them.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        self.log().entries.clone()
    }

    /// Releases the held call `call_id`, which then answers as its tool's [`Script::Hold`] says.
    /// False, releasing nothing, when the mock holds no call of that identity or released it
    /// already.
    #[must_use]
    pub fn release(&self, call_id: ToolCallId) -> bool {
        let waker = {
            let mut log = self.log();
            match log.held.get_mut(&call_id) {
                Some(held) if !held.released => {
                    held.released = true;
                    held.waker.take()
                }
                _ => return false,
            }
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        true
    }

    /// The execution of the held `call`, answering `answer` once released.
    fn held<'a>(
        &'a self,
        call: &ToolCall,
        answer: &Result<Json, ToolFailure>,
        cancellation: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        let call_id = call.call_id();
        self.log().held.entry(call_id).or_default();
        let answer = answer.clone();
        Box::pin(future::poll_fn(move |cx| {
            let mut log = self.log();
            let Log { entries, held } = &mut *log;
            let held = held.entry(call_id).or_default();
            if held.released {
                entries.push(Entry::Released(call_id, answer.clone()));
                return Poll::Ready(answer.clone());
            }
            if cancellation.is_cancelled() && !held.cancelled {
                held.cancelled = true;
                entries.push(Entry::Cancelled(call_id));
            }
            held.waker = Some(cx.waker().clone());
            Poll::Pending
        }))
    }
}

impl ToolExecutor for MockToolExecutor {
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        deadline: Instant,
        cancellation: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        let earlier = {
            let mut log = self.log();
            let earlier = log
                .entries
                .iter()
                .filter(|entry| {
                    matches!(entry, Entry::Invoked(invocation) if invocation.call_id == call.call_id())
                })
                .count();
            log.entries.push(Entry::Invoked(Invocation {
                call_id: call.call_id(),
                tool_id: call.tool_id(),
                generation_id: call.generation_id(),
                arguments: call.arguments().clone(),
            }));
            earlier
        };
        match self.table.get(&call.tool_id()) {
            Some(Script::Result(result)) => Box::pin(future::ready(Ok(result.clone()))),
            Some(Script::Failure(failure)) => Box::pin(future::ready(Err(failure.clone()))),
            Some(Script::FailThenSucceed {
                failures,
                failure,
                result,
            }) => Box::pin(future::ready(if earlier < *failures {
                Err(failure.clone())
            } else {
                Ok(result.clone())
            })),
            Some(Script::Hold(answer)) => self.held(call, answer, cancellation),
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

    use super::{Entry, MockToolExecutor, Script};

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

    #[test]
    fn a_call_scripted_to_fail_twice_fails_two_attempts_then_succeeds_and_each_call_counts_alone() {
        let flaky = tool("flaky");
        let failure = ToolFailure {
            kind: ToolFailureKind::ExecutionError,
            detail: "hiccup".to_owned(),
        };
        let mock = MockToolExecutor::new([(
            flaky.tool_id(),
            Script::FailThenSucceed {
                failures: 2,
                failure: failure.clone(),
                result: json!({"ok": true}),
            },
        )]);
        let signal = CancellationSignal::new();
        let later = Instant::now() + Duration::from_secs(60);
        let (first, second) = (proposed(&flaky), proposed(&flaky));
        let answers = [&first, &first, &second, &first, &first].map(|call| {
            let Poll::Ready(answer) = poll(mock.execute(call, later, &signal)) else {
                panic!("the script answers at once");
            };
            answer
        });
        assert_eq!(
            answers,
            [
                Err(failure.clone()),
                Err(failure.clone()),
                Err(failure),
                Ok(json!({"ok": true})),
                Ok(json!({"ok": true})),
            ]
        );
        assert_eq!(mock.invocations().len(), 5, "every attempt is recorded");
    }

    #[test]
    fn a_held_call_logs_its_cancellation_once_and_answers_only_once_released() {
        let (held, failing) = (tool("held"), tool("failing"));
        let failure = ToolFailure {
            kind: ToolFailureKind::ExecutionError,
            detail: "down".to_owned(),
        };
        let mock = MockToolExecutor::new([
            (held.tool_id(), Script::Hold(Ok(json!({"ok": true})))),
            (failing.tool_id(), Script::Hold(Err(failure.clone()))),
        ]);
        let later = Instant::now() + Duration::from_secs(60);
        let (call, other) = (proposed(&held), proposed(&failing));
        assert!(
            !mock.release(call.call_id()),
            "nothing is held before it is invoked"
        );
        let signal = CancellationSignal::new();
        let mut execution = mock.execute(&call, later, &signal);
        let mut context = Context::from_waker(Waker::noop());
        assert!(execution.as_mut().poll(&mut context).is_pending());
        signal.cancel();
        assert!(
            execution.as_mut().poll(&mut context).is_pending(),
            "cancelled, still held"
        );
        assert!(execution.as_mut().poll(&mut context).is_pending());
        assert!(mock.release(call.call_id()));
        assert!(!mock.release(call.call_id()), "released once");
        assert_eq!(
            execution.as_mut().poll(&mut context),
            Poll::Ready(Ok(json!({"ok": true})))
        );
        let uncancelled = CancellationSignal::new();
        let fails = mock.execute(&other, later, &uncancelled);
        assert!(mock.release(other.call_id()));
        assert_eq!(poll(fails), Poll::Ready(Err(failure.clone())));
        let [invoked, other_invoked] = [&call, &other].map(|call| {
            mock.invocations()
                .into_iter()
                .find(|invocation| invocation.call_id == call.call_id())
                .map(Entry::Invoked)
                .expect("each call was invoked")
        });
        assert_eq!(
            mock.entries(),
            [
                invoked,
                Entry::Cancelled(call.call_id()),
                Entry::Released(call.call_id(), Ok(json!({"ok": true}))),
                other_invoked,
                Entry::Released(other.call_id(), Err(failure)),
            ],
            "one cancelled entry, before the answer, and none for the call never cancelled"
        );
    }
}
