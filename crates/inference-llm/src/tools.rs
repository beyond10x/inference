//! Tool calling between the graph and `llm`, behind the `tools` feature
//! (`docs/design/inference.md` 237).
//!
//! A model that claims `inference.model.Capability::ToolCalling` may end a turn with `llm` tool
//! calls instead of, or after, its prose. The calls never reach the caller as
//! [`TextDelta`](inference_text::TextDelta)s: they are taken from the handle with
//! [`GenerationHandle::take_proposal`](crate::GenerationHandle::take_proposal), so structured
//! proposals stay distinguishable from user-visible prose (374). The caller turns each into an
//! `inference.tools.ProposeToolCall` of the generation that made it, of the tool [`tool_id`]
//! names.
//!
//! This module maps the two vocabularies onto each other and defines neither side. A declared
//! [`Tool`] is offered to the model as the `llm` [`ToolSpec`] [`tool_spec`] makes of it, named by
//! its [`ToolId`] so that a call names exactly one tool; what became of a call, a
//! [`ToolOutcome`], returns to the generation that proposed as the output of an `llm` tool result
//! ([`ToolOutcome::output`], [`ToolOutcome::failed`]), through
//! [`GenerationHandle::integrate`](crate::GenerationHandle::integrate). An ordinary text model
//! integrates a tool result at a safe boundary only, the end of the turn that proposed (402, 721):
//! the next turn runs once every call of the round is answered. The generation stays Active from
//! the proposal to its completion, so every segment emitted after the outcome carries the
//! generation's identity (376).

use inference_tools::{Json, RejectionReason, Tool, ToolFailure, ToolId};
use llm_core::{ToolName, ToolSpec};
use serde_json::{Value, json};

/// The name `tool_id` is offered to the model under: its canonical hyphenated form.
///
/// # Panics
///
/// Never: the hyphenated form of an identifier is 36 printable ASCII bytes, which every `llm`
/// tool name admits.
#[must_use]
pub fn tool_name(tool_id: ToolId) -> ToolName {
    ToolName::new(tool_id.to_string()).expect("a hyphenated identifier is a valid tool name")
}

/// The tool a call named `name` proposes to call, when `name` is one [`tool_name`] gives; none
/// otherwise, a name no declared tool was offered under.
#[must_use]
pub fn tool_id(name: &ToolName) -> Option<ToolId> {
    serde_json::from_value(Value::String(name.as_str().to_owned()))
        .ok()
        .filter(|tool_id| tool_name(*tool_id) == *name)
}

/// `tool` as the model is offered it: named by its [`ToolId`], described by its name, its schema
/// the input schema.
#[must_use]
pub fn tool_spec(tool: &Tool) -> ToolSpec {
    ToolSpec {
        name: tool_name(tool.tool_id()),
        description: tool.name().to_owned(),
        input_schema: tool.schema().clone(),
    }
}

/// What became of a proposal, as the tools domain reports it to the generation that made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutcome {
    /// The call completed with this result (`inference.tools.ToolCallResult`).
    Result(Json),
    /// The call started and failed (`inference.tools.ToolCallFailed`).
    Failed(ToolFailure),
    /// The call was rejected before it started (`inference.tools.ToolCallRejected`).
    Rejected(RejectionReason),
}

impl ToolOutcome {
    /// The output of the `llm` tool result: the result itself, `{"failure": <ToolFailure>}` or
    /// `{"rejected": <RejectionReason>}`, each serialized under its ESS names. A call rejected
    /// before it ran is sent as `llm` sends every such call, a failed result whose output is the
    /// caller's own text of the reason; no `llm` wire carries a separate rejected state, and the
    /// tools domain keeps `ToolCallFailed` and `ToolCallRejected` apart.
    #[must_use]
    pub fn output(&self) -> Value {
        match self {
            Self::Result(result) => result.clone(),
            Self::Failed(failure) => json!({ "failure": failure }),
            Self::Rejected(reason) => json!({ "rejected": reason }),
        }
    }

    /// Whether the `llm` tool result is a failure: the call failed or was rejected, as `llm`
    /// sends both.
    #[must_use]
    pub const fn failed(&self) -> bool {
        !matches!(self, Self::Result(_))
    }
}
