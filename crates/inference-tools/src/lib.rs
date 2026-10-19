//! The tool vocabulary of the inference framework.
//!
//! `inference-tools` holds `ess/domains/tools.yaml`: a [`Tool`] is an external capability with an
//! identity, a schema and its revision, a [`SideEffectClass`] and whether human confirmation is
//! required; a [`ToolCall`] is a structured proposal a generation made, correlated to that
//! [`Generation`](inference_core::generation::Generation), classified by its tool's side-effect
//! class and resting on a named inbound [`Span`](inference_text::Span).
//!
//! A call is proposed on one of two paths. [`ToolCall::propose_as`] is the default: the span it
//! rests on must be Committed, whatever the tool's class, since a speculative or provisional
//! transcript must not directly initiate side-effecting tool execution.
//! [`ToolCall::propose_speculative_as`] is the explicit speculative path: the span's state is not
//! read, and a tool that is not read-only is refused. Both refuse a generation that is not Active.
//!
//! A proposed call is then authorized, or rejected with a [`RejectionReason`]; authorization
//! refuses a call whose authority requires confirmation and got none ([`ConfirmationRequired`]).
//! An authorized call is started, and a started call completes with a result or fails with a
//! [`ToolFailure`]. Completed, Failed and Rejected are terminal: a tool retry is an attempt inside
//! Started made by the executor, and a generation retry is a new call. Each transition is a
//! method that returns the moved call and the event its command emits, or the error the command
//! declares; a refused move changes nothing. Which tool, generation, span or call an identity
//! names is decided by a store holding them.
//!
//! [`ToolExecutor`] is the executor contract: it executes one started call under a deadline and a
//! [`CancellationSignal`], and returns a result or a typed failure. Its caller attempts a call that
//! failed transiently again by the [`RetryPolicy`] of the call's side-effect class, under the same
//! `call_id`, and a non-idempotent write once. Stopping a model rolls back no completed write, so
//! nothing here undoes a result.
//!
//! The crate depends on `inference-core` and `inference-text`, whose [`SpanId`] and
//! [`SpanState`] the proposal reads, and carries no executor, node or other concrete
//! implementation. Serialized names are the ESS names.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use inference_core::id::GenerationId;
use inference_text::{SpanId, SpanState};

mod call;
mod executor;

pub use call::{AuthorizeError, ProposeError, ProposeSpeculativeError, ToolCall};
pub use executor::{CancellationSignal, ExecutionFuture, RetryPolicy, ToolExecutor};

/// An ESS `Json` value: a tool's schema, a call's arguments, a call's result. It is held unread.
pub type Json = serde_json::Value;

inference_core::uuid_newtype!(
    /// `inference.tools.ToolId`: identifies a [`Tool`]. It is declared as the identifiers of
    /// `inference_core::id` are, and like them is written in the canonical hyphenated form and
    /// read only in that form.
    ToolId
);

inference_core::uuid_newtype!(
    /// `inference.tools.ToolCallId`: identifies a [`ToolCall`], written and read as [`ToolId`] is.
    ToolCallId
);

/// `inference.tools.SideEffectClass`: what invoking a tool may change. Serialized as written in
/// ESS (`read_only`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectClass {
    /// Retrieves data and changes nothing.
    ReadOnly,
    /// Changes something, and repeating it changes nothing more.
    IdempotentWrite,
    /// Changes something, and repeating it changes it again.
    NonIdempotentWrite,
}

impl SideEffectClass {
    /// Every class, in the order `ess/domains/tools.yaml` lists them.
    pub const ALL: [Self; 3] = [
        Self::ReadOnly,
        Self::IdempotentWrite,
        Self::NonIdempotentWrite,
    ];
}

/// `inference.tools.RejectionReason`: why a proposed or authorized call was rejected. Serialized as
/// written in ESS (`schema_invalid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionReason {
    /// The arguments fail the tool's schema.
    SchemaInvalid,
    /// The authority denied the call.
    Denied,
    /// The generation that proposed the call was superseded.
    Superseded,
    /// A feedback loop's iteration rule is exhausted.
    Exhausted,
}

impl RejectionReason {
    /// Every reason, in the order `ess/domains/tools.yaml` lists them.
    pub const ALL: [Self; 4] = [
        Self::SchemaInvalid,
        Self::Denied,
        Self::Superseded,
        Self::Exhausted,
    ];
}

/// `inference.tools.ToolFailureKind`: how a started call failed. Serialized as written in ESS
/// (`deadline_exceeded`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailureKind {
    /// The call's deadline passed before it finished.
    DeadlineExceeded,
    /// The call was cancelled before it finished.
    Cancelled,
    /// The executor reported an error.
    ExecutionError,
}

impl ToolFailureKind {
    /// Every kind, in the order `ess/domains/tools.yaml` lists them.
    pub const ALL: [Self; 3] = [
        Self::DeadlineExceeded,
        Self::Cancelled,
        Self::ExecutionError,
    ];
}

/// `inference.tools.ToolFailure`: the typed, actionable failure of a started call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolFailure {
    pub kind: ToolFailureKind,
    pub detail: String,
}

impl fmt::Display for ToolFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the tool call failed ({:?}): {}", self.kind, self.detail)
    }
}

impl Error for ToolFailure {}

/// `inference.tools.ToolCall.State`: where a call stands. Serialized as written in ESS
/// (`Proposed`, `Rejected`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolCallState {
    /// Proposed by a generation; it awaits authorization.
    Proposed,
    /// Authorized; it awaits the executor.
    Authorized,
    /// The executor is running it.
    Started,
    /// It completed with a result.
    Completed,
    /// It failed with a typed failure.
    Failed,
    /// It was rejected with a reason and never executed.
    Rejected,
}

impl ToolCallState {
    /// Every state, in the order `ess/domains/tools.yaml` lists them.
    pub const ALL: [Self; 6] = [
        Self::Proposed,
        Self::Authorized,
        Self::Started,
        Self::Completed,
        Self::Failed,
        Self::Rejected,
    ];

    /// Whether no transition leaves the state: Completed, Failed and Rejected.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Rejected)
    }
}

/// `inference.tools.DeclareTool`: the input that declares a [`Tool`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclareTool {
    pub name: String,
    pub schema: Json,
    pub schema_revision: String,
    pub side_effect: SideEffectClass,
    pub requires_confirmation: bool,
}

/// `inference.tools.ProposeToolCall`: the input of the default proposal path, naming the tool, the
/// generation proposing and the span the proposal rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposeToolCall {
    pub tool_id: ToolId,
    pub generation_id: GenerationId,
    pub basis_span_id: SpanId,
    pub arguments: Json,
}

/// `inference.tools.ProposeSpeculativeToolCall`: the input of the explicit speculative path, with
/// the fields of [`ProposeToolCall`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposeSpeculativeToolCall {
    pub tool_id: ToolId,
    pub generation_id: GenerationId,
    pub basis_span_id: SpanId,
    pub arguments: Json,
}

/// `inference.tools.AuthorizeToolCall`: the authority's decision to admit the call `call_id`
/// names. `confirmation_required` is whether its policy, having read the tool's
/// `requires_confirmation`, requires human confirmation; `confirmed` is the confirmation hook's
/// answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizeToolCall {
    pub call_id: ToolCallId,
    pub confirmation_required: bool,
    pub confirmed: bool,
}

impl AuthorizeToolCall {
    /// The confirmation hook: refuses when confirmation is required and none was given. It reads
    /// the input only, so it is decided before the call is looked up, as the specification orders
    /// `unconfirmed` before `wrong-state`.
    ///
    /// # Errors
    ///
    /// [`ConfirmationRequired`] naming `call_id`.
    pub const fn confirmation(&self) -> Result<(), ConfirmationRequired> {
        if self.confirmation_required && !self.confirmed {
            return Err(ConfirmationRequired {
                call_id: self.call_id,
            });
        }
        Ok(())
    }
}

/// `inference.tools.RejectToolCall`: rejects the call `call_id` names with `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectToolCall {
    pub call_id: ToolCallId,
    pub reason: RejectionReason,
}

/// `inference.tools.StartToolCall`: the executor starts the call `call_id` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartToolCall {
    pub call_id: ToolCallId,
}

/// `inference.tools.CompleteToolCall`: the call `call_id` names completed with `result`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteToolCall {
    pub call_id: ToolCallId,
    pub result: Json,
}

/// `inference.tools.FailToolCall`: the call `call_id` names failed with `failure`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailToolCall {
    pub call_id: ToolCallId,
    pub failure: ToolFailure,
}

/// `inference.tools.ToolDeclared`: the tool is known with its schema, schema revision and class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDeclared {
    pub tool_id: ToolId,
    pub name: String,
    pub side_effect: SideEffectClass,
}

/// `inference.tools.ToolCallProposed`: a generation proposed a call; it awaits authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallProposed {
    pub call_id: ToolCallId,
    pub tool_id: ToolId,
    pub generation_id: GenerationId,
}

/// `inference.tools.ToolCallAuthorized`: the call may be executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallAuthorized {
    pub call_id: ToolCallId,
}

/// `inference.tools.ToolCallRejected`: the call is rejected with its reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRejected {
    pub call_id: ToolCallId,
    pub reason: RejectionReason,
}

/// `inference.tools.ToolCallStarted`: the executor is running the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallStarted {
    pub call_id: ToolCallId,
}

/// `inference.tools.ToolCallResult`: the call completed with `result`, for correlated context
/// reinjection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub call_id: ToolCallId,
    pub result: Json,
}

/// `inference.tools.ToolCallFailed`: the call failed with `failure`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallFailed {
    pub call_id: ToolCallId,
    pub failure: ToolFailure,
}

/// `inference.tools.ProvisionalInputBarrier`: a speculative or provisional transcript must not
/// directly initiate side-effecting tool execution; the proposal is refused. `side_effect` is the
/// class of the tool proposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionalInputBarrier {
    pub side_effect: SideEffectClass,
}

impl fmt::Display for ProvisionalInputBarrier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a {:?} tool may not act on provisional input; nothing was proposed",
            self.side_effect
        )
    }
}

impl Error for ProvisionalInputBarrier {}

/// `inference.tools.ToolNotFound`: no tool carries the identity a proposal named. `tool_id` is the
/// identity that was sent. Only a store holding the tools can decide it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolNotFound {
    pub tool_id: ToolId,
}

impl fmt::Display for ToolNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no tool carries the identity {}", self.tool_id)
    }
}

impl Error for ToolNotFound {}

/// `inference.tools.ToolCallStateConflict`: the call is not in a state the command moves it from,
/// so nothing moved. `current` is the state it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallStateConflict {
    pub current: ToolCallState,
}

impl fmt::Display for ToolCallStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the call is {:?}, which this command does not move it from, so nothing moved",
            self.current
        )
    }
}

impl Error for ToolCallStateConflict {}

/// `inference.tools.ConfirmationRequired`: confirmation is required and none was given; the call
/// stays Proposed. `call_id` is the identity that was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmationRequired {
    pub call_id: ToolCallId,
}

impl fmt::Display for ConfirmationRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the call {} requires human confirmation and none was given",
            self.call_id
        )
    }
}

impl Error for ConfirmationRequired {}

/// `inference.tools.Tool`: an external capability with a schema and a side-effect class. It has
/// one state, Declared, and is never changed; redeclaring a tool declares a new one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Tool {
    tool_id: ToolId,
    name: String,
    schema: Json,
    schema_revision: String,
    side_effect: SideEffectClass,
    requires_confirmation: bool,
}

impl Tool {
    /// Runs `inference.tools.DeclareTool`: generates a [`ToolId`] and returns the tool and its
    /// [`ToolDeclared`].
    #[must_use]
    pub fn declare(input: DeclareTool) -> (Self, ToolDeclared) {
        Self::declare_as(ToolId::generate(), input)
    }

    /// Runs `inference.tools.DeclareTool` under `tool_id` instead of a generated one. It is for a
    /// store that assigns the identities itself, such as the counter of `inference-testkit`.
    #[must_use]
    pub fn declare_as(tool_id: ToolId, input: DeclareTool) -> (Self, ToolDeclared) {
        let event = ToolDeclared {
            tool_id,
            name: input.name.clone(),
            side_effect: input.side_effect,
        };
        let tool = Self {
            tool_id,
            name: input.name,
            schema: input.schema,
            schema_revision: input.schema_revision,
            side_effect: input.side_effect,
            requires_confirmation: input.requires_confirmation,
        };
        (tool, event)
    }

    #[must_use]
    pub const fn tool_id(&self) -> ToolId {
        self.tool_id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn schema(&self) -> &Json {
        &self.schema
    }

    #[must_use]
    pub fn schema_revision(&self) -> &str {
        &self.schema_revision
    }

    #[must_use]
    pub const fn side_effect(&self) -> SideEffectClass {
        self.side_effect
    }

    /// Whether the tool requires human confirmation; the authority reads it when it authorizes a
    /// call.
    #[must_use]
    pub const fn requires_confirmation(&self) -> bool {
        self.requires_confirmation
    }
}

/// Whether a span in `state` is committed input a side-effecting call may rest on.
const fn committed(state: SpanState) -> bool {
    matches!(state, SpanState::Committed)
}
