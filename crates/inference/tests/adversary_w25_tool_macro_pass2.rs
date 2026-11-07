//! Adversarial cases for the correction of `#[inference::tool]` and the tool authority: the
//! macro's promise that every subschema is inlined, and the authority's promise that a keyword it
//! reads in a form other than JSON Schema's fails preparation.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference::core::envelope::ControlKind;
use inference::core::id::RunId;
use inference::pipeline::NodeImplementation;
use inference::pipeline::nodes::{ToolAuthority, ToolCallUpdate, ToolPayload};
use inference::tools::{DeclareTool, SideEffectClass, Tool};
use serde_json::json;

/// A node of an outline, holding the nodes under it.
#[derive(inference::__private::serde::Deserialize, inference::__private::schemars::JsonSchema)]
#[serde(crate = "inference::__private::serde")]
#[schemars(crate = "inference::__private::schemars")]
struct Outline {
    title: String,
    children: Vec<Outline>,
}

/// Counts the nodes of an outline.
#[inference::tool]
async fn count_nodes(outline: Outline) -> usize {
    fn count(outline: &Outline) -> usize {
        let _ = &outline.title;
        1 + outline.children.iter().map(count).sum::<usize>()
    }
    count(&outline)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Payload {
    Tool(ToolCallUpdate),
    Control(ControlKind),
}

impl From<ControlKind> for Payload {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl ToolPayload for Payload {
    fn into_tool_call(self) -> Result<ToolCallUpdate, Self> {
        match self {
            Self::Tool(update) => Ok(update),
            other @ Self::Control(_) => Err(other),
        }
    }

    fn from_tool_call(update: ToolCallUpdate) -> Self {
        Self::Tool(update)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
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

fn prepared(tool: Tool) -> Result<(), String> {
    let mut authority = ToolAuthority::new(RunId::generate(), [tool]);
    block_on(NodeImplementation::<Payload>::prepare(&mut authority)).map_err(|e| e.to_string())
}

/// The macro documents that non-recursive subschemas are inlined and a recursive argument keeps
/// `$defs` and `$ref`, which the tool authority refuses at preparation: schemars inlines a type
/// only while it is not already being generated, so a self-referential argument keeps both.
#[test]
fn a_self_referential_argument_fails_preparation() {
    let (tool, _) = count_nodes();
    let schema = tool.schema().to_string();
    assert!(
        schema.contains("$ref") && schema.contains("$defs"),
        "{schema}"
    );
    assert!(
        prepared(tool).is_err(),
        "a recursive schema was prepared with"
    );
}

fn annotated(keyword: &str, value: serde_json::Value) -> Tool {
    Tool::declare(DeclareTool {
        name: "at".to_owned(),
        description: "Something at a time.".to_owned(),
        schema: json!({
            "type": "object",
            "properties": {"when": {"type": "string", keyword: value}},
        }),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// JSON Schema's `title` is a string.
#[test]
fn a_title_that_is_not_a_string_fails_preparation() {
    assert!(prepared(annotated("title", json!(5))).is_err());
    assert!(prepared(annotated("title", json!({"text": "When"}))).is_err());
    assert_eq!(prepared(annotated("title", json!("When"))), Ok(()));
}

/// JSON Schema's `description` is a string.
#[test]
fn a_description_that_is_not_a_string_fails_preparation() {
    assert!(prepared(annotated("description", json!(["the time"]))).is_err());
    assert!(prepared(annotated("description", json!(null))).is_err());
    assert_eq!(
        prepared(annotated("description", json!("The time."))),
        Ok(())
    );
}

/// The authority documents that a tool whose schema uses a keyword it reads "in a form other than
/// JSON Schema's" fails preparation. JSON Schema's `format` is a string; the correction admits
/// `format` with any value.
#[test]
fn a_format_that_is_not_a_string_fails_preparation() {
    let (tool, _) = Tool::declare(DeclareTool {
        name: "at".to_owned(),
        description: "Something at a time.".to_owned(),
        schema: json!({
            "type": "object",
            "properties": {"when": {"type": "string", "format": 5}},
        }),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    });
    assert!(prepared(tool).is_err(), "a `format` of 5 was prepared with");
}
