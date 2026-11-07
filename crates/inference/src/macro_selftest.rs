//! The macros used inside this crate, through the `::inference::` paths they emit.
//!
//! A unit test here expands `#[inference::tool]` inside `inference` itself, where its paths
//! resolve through `extern crate self as inference;`. With the feature `audio-device`, the
//! example below compiles `#[inference::main]`; it is `no_run`, so it opens no device and reads
//! no configuration.
#![cfg_attr(
    feature = "audio-device",
    doc = r#"
```no_run
use inference::prelude::*;

#[inference::main]
async fn main(host: impl AudioHost, stack: Stack) {
    let _ = (host, stack);
}
```
"#
)]

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread::{self, Thread};
    use std::time::{Duration, Instant};

    use inference_core::generation::{Generation, StartGeneration};
    use inference_core::id::{NodeId, RunId, StreamId};
    use inference_text::{OpenTurn, Span, Turn};
    use serde_json::json;

    use crate::tools::{AuthorizeToolCall, CancellationSignal, Tool, ToolCall, ToolExecutor};

    /// Multiplies a word's length by a factor.
    #[inference::tool]
    async fn scaled_length(word: String, factor: u32) -> usize {
        word.len() * factor as usize
    }

    struct Unpark(Thread);

    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut future = pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
                return output;
            }
            thread::park();
        }
    }

    fn started(tool: &Tool, arguments: serde_json::Value) -> ToolCall {
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
        let (call, _) = ToolCall::propose(tool, &generation, &span, arguments)
            .expect("an Active generation proposes on a committed span");
        let (call, _) = call
            .authorize(&AuthorizeToolCall {
                call_id: call.call_id(),
                confirmation_required: false,
                confirmed: false,
            })
            .expect("Proposed authorizes");
        call.start().expect("Authorized starts").0
    }

    #[test]
    fn the_tool_macro_expands_inside_the_crate_into_a_tool_and_its_executor() {
        let (tool, executor): (Tool, Box<dyn ToolExecutor>) = scaled_length();
        assert_eq!(tool.name(), "scaled_length");
        assert_eq!(
            tool.schema(),
            &json!({
                "type": "object",
                "properties": {
                    "word": {"type": "string"},
                    "factor": {"type": "integer", "format": "uint32", "minimum": 0}
                },
                "required": ["word", "factor"]
            })
        );
        let call = started(&tool, json!({"word": "four", "factor": 3}));
        let result = block_on(executor.execute(
            &call,
            Instant::now() + Duration::from_secs(10),
            &CancellationSignal::new(),
        ));
        assert_eq!(result, Ok(json!(12)));
    }
}
