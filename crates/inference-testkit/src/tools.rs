//! The tools and tool calls of `ess/domains/tools.yaml`, held in memory.
//!
//! [`ToolsDomain`] holds one store per entity, [`ToolStore`] and [`ToolCallStore`], and records
//! every event it emits in order. Its commands run `inference-tools`' [`Tool::declare_as`] and the
//! [`ToolCall`] state machine under identities the domain issues from a counter rendered as a Uuid
//! in its own [`IDENTITY_SPACE`], `00000004-0000-4000-8000-000000000001` first, so that two runs of
//! the same commands report the same identities and no other domain issues one of them.
//!
//! What only a store can decide is decided here. A proposal reads the generation and the span it
//! names from the `inference.stream` store, [`StreamDomain`], which [`ToolsDomain::propose_tool_call`]
//! and [`ToolsDomain::propose_speculative_tool_call`] take: a tool, then a generation, then a span
//! no record carries is refused, in that order, before a generation that is not Active and before
//! the barrier. A command naming a call no record carries is refused before anything moves;
//! `AuthorizeToolCall`'s confirmation hook reads the input alone and answers before the call is
//! looked up. The views `inference.tools.Tools` and `inference.tools.ToolCalls` are
//! [`ToolsDomain::tools_view`] and [`ToolsDomain::tool_calls_view`]; both are `read_your_writes`,
//! and every row is read from the stores at the call.
//!
//! [`MockToolExecutor`] is the scripted mock of the executor contract of `inference-tools`: a
//! table of tools, each with a [`Script`], and a log of every [`Invocation`] it received.
//!
//! [`Tool::declare_as`]: inference_tools::Tool::declare_as
//! [`ToolCall`]: inference_tools::ToolCall
//! [`StreamDomain`]: crate::stream::StreamDomain

use std::error::Error;
use std::fmt;

use uuid::Uuid;

use inference_core::generation::{GenerationNotFound, GenerationStateConflict};
use inference_core::id::GenerationId;
use inference_text::{SpanId, SpanNotFound};
use inference_tools::{
    AuthorizeError, AuthorizeToolCall, CompleteToolCall, ConfirmationRequired, DeclareTool,
    FailToolCall, Json, ProposeError, ProposeSpeculativeError, ProposeSpeculativeToolCall,
    ProposeToolCall, ProvisionalInputBarrier, RejectToolCall, SideEffectClass, Tool, ToolCall,
    ToolCallAuthorized, ToolCallFailed, ToolCallId, ToolCallProposed, ToolCallRejected,
    ToolCallResult, ToolCallStarted, ToolCallState, ToolCallStateConflict, ToolDeclared, ToolId,
    ToolNotFound,
};

use crate::stream::StreamDomain;

/// The first eight hex digits of every identity the domain issues, which no other domain of the
/// testkit issues under (see [`crate::policy::IDENTITY_SPACE`]).
pub const IDENTITY_SPACE: u32 = 4;

/// The store of `inference.tools.Tool`, in the order the tools were declared.
#[derive(Debug, Clone, Default)]
pub struct ToolStore {
    tools: Vec<Tool>,
}

impl ToolStore {
    /// The tool carrying `tool_id`, if one was declared.
    #[must_use]
    pub fn get(&self, tool_id: ToolId) -> Option<&Tool> {
        self.tools.iter().find(|tool| tool.tool_id() == tool_id)
    }

    /// Every tool, in the order they were declared.
    pub fn iter(&self) -> impl Iterator<Item = &Tool> {
        self.tools.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// The store of `inference.tools.ToolCall`, in the order the calls were proposed. A call stays in
/// every state, terminal ones included: the store is the audit record.
#[derive(Debug, Clone, Default)]
pub struct ToolCallStore {
    calls: Vec<ToolCall>,
}

impl ToolCallStore {
    /// The call carrying `call_id`, if one was proposed.
    #[must_use]
    pub fn get(&self, call_id: ToolCallId) -> Option<&ToolCall> {
        self.calls.iter().find(|call| call.call_id() == call_id)
    }

    /// Every call, in the order they were proposed.
    pub fn iter(&self) -> impl Iterator<Item = &ToolCall> {
        self.calls.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.calls.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

/// An event the domain emitted, in the order it emitted them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolsEvent {
    ToolDeclared(ToolDeclared),
    ToolCallProposed(ToolCallProposed),
    ToolCallAuthorized(ToolCallAuthorized),
    ToolCallRejected(ToolCallRejected),
    ToolCallStarted(ToolCallStarted),
    ToolCallResult(ToolCallResult),
    ToolCallFailed(ToolCallFailed),
}

/// Why `inference.tools.ProposeToolCall` proposed nothing, in the order
/// [`ToolsDomain::propose_tool_call`] decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposeToolCallError {
    /// The `unknown-tool` outcome.
    UnknownTool(ToolNotFound),
    /// The `unknown-generation` outcome.
    UnknownGeneration(GenerationNotFound),
    /// The `unknown-span` outcome.
    UnknownSpan(SpanNotFound),
    /// The `stale-generation` outcome: the generation is not Active.
    StaleGeneration(GenerationStateConflict),
    /// The `barred` outcome: the span is still a Hypothesis.
    Barred(ProvisionalInputBarrier),
}

impl fmt::Display for ProposeToolCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTool(error) => fmt::Display::fmt(error, f),
            Self::UnknownGeneration(error) => fmt::Display::fmt(error, f),
            Self::UnknownSpan(error) => fmt::Display::fmt(error, f),
            Self::StaleGeneration(error) => fmt::Display::fmt(error, f),
            Self::Barred(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ProposeToolCallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownTool(error) => Some(error),
            Self::UnknownGeneration(error) => Some(error),
            Self::UnknownSpan(error) => Some(error),
            Self::StaleGeneration(error) => Some(error),
            Self::Barred(error) => Some(error),
        }
    }
}

/// Why `inference.tools.ProposeSpeculativeToolCall` proposed nothing, in the order
/// [`ToolsDomain::propose_speculative_tool_call`] decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposeSpeculativeToolCallError {
    /// The `unknown-tool` outcome.
    UnknownTool(ToolNotFound),
    /// The `unknown-generation` outcome.
    UnknownGeneration(GenerationNotFound),
    /// The `unknown-span` outcome.
    UnknownSpan(SpanNotFound),
    /// The `stale-generation` outcome: the generation is not Active.
    StaleGeneration(GenerationStateConflict),
    /// The `side-effecting` outcome: the tool is not read-only.
    SideEffecting(ProvisionalInputBarrier),
}

impl fmt::Display for ProposeSpeculativeToolCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTool(error) => fmt::Display::fmt(error, f),
            Self::UnknownGeneration(error) => fmt::Display::fmt(error, f),
            Self::UnknownSpan(error) => fmt::Display::fmt(error, f),
            Self::StaleGeneration(error) => fmt::Display::fmt(error, f),
            Self::SideEffecting(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ProposeSpeculativeToolCallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownTool(error) => Some(error),
            Self::UnknownGeneration(error) => Some(error),
            Self::UnknownSpan(error) => Some(error),
            Self::StaleGeneration(error) => Some(error),
            Self::SideEffecting(error) => Some(error),
        }
    }
}

/// Why a call command moved nothing. Both are the command's `wrong-state` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallMoveError {
    /// No stored call carries the identity sent; there is no state to report.
    Unknown { call_id: ToolCallId },
    /// The call is not in a state the command moves it from.
    Conflict(ToolCallStateConflict),
}

impl fmt::Display for ToolCallMoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { call_id } => write!(f, "no call carries the identity {call_id}"),
            Self::Conflict(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ToolCallMoveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unknown { .. } => None,
            Self::Conflict(error) => Some(error),
        }
    }
}

/// Why `inference.tools.AuthorizeToolCall` moved nothing, in the order
/// [`ToolsDomain::authorize_tool_call`] decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizeToolCallError {
    /// The `unconfirmed` outcome, decided from the input before the call is looked up.
    Unconfirmed(ConfirmationRequired),
    /// The `wrong-state` outcome with no call to report a state of.
    Unknown { call_id: ToolCallId },
    /// The `wrong-state` outcome: the call is not Proposed.
    Conflict(ToolCallStateConflict),
}

impl fmt::Display for AuthorizeToolCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfirmed(error) => fmt::Display::fmt(error, f),
            Self::Unknown { call_id } => write!(f, "no call carries the identity {call_id}"),
            Self::Conflict(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for AuthorizeToolCallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unconfirmed(error) => Some(error),
            Self::Unknown { .. } => None,
            Self::Conflict(error) => Some(error),
        }
    }
}

/// A row of the view `inference.tools.Tools`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolsRow {
    pub tool_id: ToolId,
    pub name: String,
    pub schema: Json,
    pub schema_revision: String,
    pub side_effect: SideEffectClass,
    pub requires_confirmation: bool,
}

/// A row of the view `inference.tools.ToolCalls`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallsRow {
    pub call_id: ToolCallId,
    pub tool_id: ToolId,
    pub generation_id: GenerationId,
    pub side_effect: SideEffectClass,
    pub basis_span_id: SpanId,
    pub arguments: Json,
    pub state: ToolCallState,
}

/// `inference.tools` in memory: its two entity stores and the events it emitted.
#[derive(Debug, Clone, Default)]
pub struct ToolsDomain {
    tools: ToolStore,
    calls: ToolCallStore,
    events: Vec<ToolsEvent>,
    issued: u128,
}

impl ToolsDomain {
    /// An empty domain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `inference.tools.DeclareTool`: stores the tool [`Tool::declare_as`] returns and
    /// records its [`ToolDeclared`].
    pub fn declare_tool(&mut self, input: DeclareTool) -> ToolDeclared {
        let tool_id = ToolId::from(self.issue());
        let (tool, event) = Tool::declare_as(tool_id, input);
        self.tools.tools.push(tool);
        self.events.push(ToolsEvent::ToolDeclared(event.clone()));
        event
    }

    /// Runs `inference.tools.ProposeToolCall` against the generations and spans `stream` holds:
    /// an identity no record carries is refused, tool, then generation, then span, and otherwise
    /// the call is proposed as [`ToolCall::propose_as`] proposes it, resting on the span's held
    /// state.
    ///
    /// # Errors
    ///
    /// In this order: [`ProposeToolCallError::UnknownTool`], [`ProposeToolCallError::UnknownGeneration`],
    /// [`ProposeToolCallError::UnknownSpan`], [`ProposeToolCallError::StaleGeneration`],
    /// [`ProposeToolCallError::Barred`]. Nothing is stored, no event is recorded and no identity
    /// is issued.
    pub fn propose_tool_call(
        &mut self,
        stream: &StreamDomain,
        input: ProposeToolCall,
    ) -> Result<ToolCallProposed, ProposeToolCallError> {
        let tool = self
            .tools
            .get(input.tool_id)
            .ok_or(ProposeToolCallError::UnknownTool(ToolNotFound {
                tool_id: input.tool_id,
            }))?;
        let generation = stream.generations().get(input.generation_id).ok_or(
            ProposeToolCallError::UnknownGeneration(GenerationNotFound {
                generation_id: input.generation_id,
            }),
        )?;
        let basis =
            stream
                .spans()
                .get(input.basis_span_id)
                .ok_or(ProposeToolCallError::UnknownSpan(SpanNotFound {
                    span_id: input.basis_span_id,
                }))?;
        let call_id = ToolCallId::from(self.next());
        let (call, event) = ToolCall::propose_as(call_id, tool, generation, basis, input.arguments)
            .map_err(|error| match error {
                ProposeError::StaleGeneration(conflict) => {
                    ProposeToolCallError::StaleGeneration(conflict)
                }
                ProposeError::Barred(barrier) => ProposeToolCallError::Barred(barrier),
            })?;
        Ok(self.proposed(call, event))
    }

    /// Runs `inference.tools.ProposeSpeculativeToolCall` against the generations and spans
    /// `stream` holds, refusing an unknown identity as [`ToolsDomain::propose_tool_call`] does,
    /// and otherwise as [`ToolCall::propose_speculative_as`]: the span's state is not read.
    ///
    /// # Errors
    ///
    /// In this order: [`ProposeSpeculativeToolCallError::UnknownTool`],
    /// [`ProposeSpeculativeToolCallError::UnknownGeneration`],
    /// [`ProposeSpeculativeToolCallError::UnknownSpan`],
    /// [`ProposeSpeculativeToolCallError::StaleGeneration`],
    /// [`ProposeSpeculativeToolCallError::SideEffecting`]. Nothing is stored, no event is recorded
    /// and no identity is issued.
    pub fn propose_speculative_tool_call(
        &mut self,
        stream: &StreamDomain,
        input: ProposeSpeculativeToolCall,
    ) -> Result<ToolCallProposed, ProposeSpeculativeToolCallError> {
        let tool =
            self.tools
                .get(input.tool_id)
                .ok_or(ProposeSpeculativeToolCallError::UnknownTool(ToolNotFound {
                    tool_id: input.tool_id,
                }))?;
        let generation = stream.generations().get(input.generation_id).ok_or(
            ProposeSpeculativeToolCallError::UnknownGeneration(GenerationNotFound {
                generation_id: input.generation_id,
            }),
        )?;
        let basis = stream.spans().get(input.basis_span_id).ok_or(
            ProposeSpeculativeToolCallError::UnknownSpan(SpanNotFound {
                span_id: input.basis_span_id,
            }),
        )?;
        let call_id = ToolCallId::from(self.next());
        let (call, event) =
            ToolCall::propose_speculative_as(call_id, tool, generation, basis, input.arguments)
                .map_err(|error| match error {
                    ProposeSpeculativeError::StaleGeneration(conflict) => {
                        ProposeSpeculativeToolCallError::StaleGeneration(conflict)
                    }
                    ProposeSpeculativeError::SideEffecting(barrier) => {
                        ProposeSpeculativeToolCallError::SideEffecting(barrier)
                    }
                })?;
        Ok(self.proposed(call, event))
    }

    /// Stores a proposed call under the identity [`ToolsDomain::next`] gave it and records its
    /// event.
    fn proposed(&mut self, call: ToolCall, event: ToolCallProposed) -> ToolCallProposed {
        self.issue();
        self.calls.calls.push(call);
        self.events.push(ToolsEvent::ToolCallProposed(event));
        event
    }

    /// Runs `inference.tools.AuthorizeToolCall`: the confirmation hook answers from the input
    /// alone, then the call `input.call_id` names moves from Proposed to Authorized.
    ///
    /// # Errors
    ///
    /// [`AuthorizeToolCallError::Unconfirmed`] when confirmation is required and none was given,
    /// whether or not a call carries the identity; then [`AuthorizeToolCallError::Unknown`] when
    /// none does; then [`AuthorizeToolCallError::Conflict`] when it is not Proposed. Nothing
    /// moves and no event is recorded.
    pub fn authorize_tool_call(
        &mut self,
        input: AuthorizeToolCall,
    ) -> Result<ToolCallAuthorized, AuthorizeToolCallError> {
        input
            .confirmation()
            .map_err(AuthorizeToolCallError::Unconfirmed)?;
        let slot = self
            .slot(input.call_id)
            .ok_or(AuthorizeToolCallError::Unknown {
                call_id: input.call_id,
            })?;
        let (moved, event) = slot.authorize(&input).map_err(|error| match error {
            AuthorizeError::Unconfirmed(error) => AuthorizeToolCallError::Unconfirmed(error),
            AuthorizeError::Conflict(conflict) => AuthorizeToolCallError::Conflict(conflict),
        })?;
        *slot = moved;
        self.events.push(ToolsEvent::ToolCallAuthorized(event));
        Ok(event)
    }

    /// Runs `inference.tools.RejectToolCall`: Proposed or Authorized to Rejected.
    ///
    /// # Errors
    ///
    /// [`ToolCallMoveError::Unknown`] when no stored call carries `call_id`,
    /// [`ToolCallMoveError::Conflict`] when it is Started or terminal.
    pub fn reject_tool_call(
        &mut self,
        input: RejectToolCall,
    ) -> Result<ToolCallRejected, ToolCallMoveError> {
        self.move_call(
            input.call_id,
            |call| call.reject(input.reason),
            ToolsEvent::ToolCallRejected,
        )
    }

    /// Runs `inference.tools.StartToolCall`: Authorized to Started.
    ///
    /// # Errors
    ///
    /// [`ToolCallMoveError::Unknown`] when no stored call carries `call_id`,
    /// [`ToolCallMoveError::Conflict`] when it is not Authorized.
    pub fn start_tool_call(
        &mut self,
        call_id: ToolCallId,
    ) -> Result<ToolCallStarted, ToolCallMoveError> {
        self.move_call(call_id, ToolCall::start, ToolsEvent::ToolCallStarted)
    }

    /// Runs `inference.tools.CompleteToolCall`: Started to Completed.
    ///
    /// # Errors
    ///
    /// [`ToolCallMoveError::Unknown`] when no stored call carries `call_id`,
    /// [`ToolCallMoveError::Conflict`] when it is not Started.
    pub fn complete_tool_call(
        &mut self,
        input: CompleteToolCall,
    ) -> Result<ToolCallResult, ToolCallMoveError> {
        let result = input.result;
        self.move_call(
            input.call_id,
            |call| call.complete(result.clone()),
            ToolsEvent::ToolCallResult,
        )
    }

    /// Runs `inference.tools.FailToolCall`: Started to Failed.
    ///
    /// # Errors
    ///
    /// [`ToolCallMoveError::Unknown`] when no stored call carries `call_id`,
    /// [`ToolCallMoveError::Conflict`] when it is not Started.
    pub fn fail_tool_call(
        &mut self,
        input: FailToolCall,
    ) -> Result<ToolCallFailed, ToolCallMoveError> {
        let failure = input.failure;
        self.move_call(
            input.call_id,
            |call| call.fail(failure.clone()),
            ToolsEvent::ToolCallFailed,
        )
    }

    fn slot(&mut self, call_id: ToolCallId) -> Option<&mut ToolCall> {
        self.calls
            .calls
            .iter_mut()
            .find(|call| call.call_id() == call_id)
    }

    fn move_call<E: Clone>(
        &mut self,
        call_id: ToolCallId,
        transition: impl FnOnce(&ToolCall) -> Result<(ToolCall, E), ToolCallStateConflict>,
        record: impl FnOnce(E) -> ToolsEvent,
    ) -> Result<E, ToolCallMoveError> {
        let slot = self
            .slot(call_id)
            .ok_or(ToolCallMoveError::Unknown { call_id })?;
        let (moved, event) = transition(slot).map_err(ToolCallMoveError::Conflict)?;
        *slot = moved;
        self.events.push(record(event.clone()));
        Ok(event)
    }

    /// The next identity of the counter, taken.
    fn issue(&mut self) -> Uuid {
        self.issued += 1;
        Self::identity(self.issued)
    }

    /// The next identity of the counter, not taken; a refused command takes none.
    fn next(&self) -> Uuid {
        Self::identity(self.issued + 1)
    }

    /// The `count`th identity: a version 4, RFC 4122 variant Uuid whose low bits are `count` and
    /// whose first eight hex digits are [`IDENTITY_SPACE`], `00000004-0000-4000-8000-000000000001`
    /// first.
    fn identity(count: u128) -> Uuid {
        Uuid::from_u128(
            (u128::from(IDENTITY_SPACE) << 96) | 0x0000_0000_0000_4000_8000_0000_0000_0000 | count,
        )
    }

    /// The declared tools.
    #[must_use]
    pub const fn tools(&self) -> &ToolStore {
        &self.tools
    }

    /// The proposed calls, in every state.
    #[must_use]
    pub const fn calls(&self) -> &ToolCallStore {
        &self.calls
    }

    /// Every event emitted, in the order it was emitted.
    #[must_use]
    pub fn events(&self) -> &[ToolsEvent] {
        &self.events
    }

    /// The view `inference.tools.Tools`: one row per tool, in the order they were declared.
    #[must_use]
    pub fn tools_view(&self) -> Vec<ToolsRow> {
        self.tools
            .iter()
            .map(|tool| ToolsRow {
                tool_id: tool.tool_id(),
                name: tool.name().to_owned(),
                schema: tool.schema().clone(),
                schema_revision: tool.schema_revision().to_owned(),
                side_effect: tool.side_effect(),
                requires_confirmation: tool.requires_confirmation(),
            })
            .collect()
    }

    /// The view `inference.tools.ToolCalls`: one row per call, in the order they were proposed.
    #[must_use]
    pub fn tool_calls_view(&self) -> Vec<ToolCallsRow> {
        self.calls
            .iter()
            .map(|call| ToolCallsRow {
                call_id: call.call_id(),
                tool_id: call.tool_id(),
                generation_id: call.generation_id(),
                side_effect: call.side_effect(),
                basis_span_id: call.basis_span_id(),
                arguments: call.arguments().clone(),
                state: call.state(),
            })
            .collect()
    }
}

/// The scripted mock of the executor contract, with its log of invocations.
mod mock_tool_executor;

pub use mock_tool_executor::{Invocation, MockToolExecutor, Script};

#[cfg(test)]
mod tests {
    use inference_core::generation::{
        GenerationNotFound, GenerationState, GenerationStateConflict, StartGeneration,
    };
    use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
    use inference_text::{EmitHypothesis, OpenTurn, SpanId, SpanNotFound};
    use inference_tools::{
        AuthorizeToolCall, CompleteToolCall, ConfirmationRequired, DeclareTool, FailToolCall,
        ProposeSpeculativeToolCall, ProposeToolCall, ProvisionalInputBarrier, RejectToolCall,
        RejectionReason, SideEffectClass, ToolCallId, ToolCallState, ToolCallStateConflict,
        ToolFailure, ToolFailureKind, ToolId, ToolNotFound,
    };
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        AuthorizeToolCallError, ProposeSpeculativeToolCallError, ProposeToolCallError,
        ToolCallMoveError, ToolsDomain, ToolsEvent,
    };
    use crate::stream::StreamDomain;

    fn run_id() -> RunId {
        RunId::from(Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d))
    }

    fn declare(domain: &mut ToolsDomain, side_effect: SideEffectClass) -> ToolId {
        domain
            .declare_tool(DeclareTool {
                name: "book".to_owned(),
                schema: json!({"type": "object"}),
                schema_revision: "1".to_owned(),
                side_effect,
                requires_confirmation: true,
            })
            .tool_id
    }

    /// A stream store with an Active generation, a Hypothesis span and a Committed span.
    fn stream() -> (StreamDomain, GenerationId, SpanId, SpanId) {
        let mut stream = StreamDomain::new();
        let generation = stream
            .start_generation(StartGeneration {
                run_id: run_id(),
                node_id: NodeId::from(Uuid::from_u128(2)),
                epoch: 1,
            })
            .generation_id;
        let turn = stream.open_turn(OpenTurn { run_id: run_id() }).turn_id;
        let mut span = |sequence| {
            stream
                .emit_hypothesis(EmitHypothesis {
                    run_id: run_id(),
                    turn_id: turn,
                    stream_id: StreamId::from(Uuid::from_u128(3)),
                    sequence,
                    text: "book it".to_owned(),
                })
                .expect("an Open turn admits a span")
                .span_id
        };
        let hypothesis = span(1);
        let committed = span(2);
        stream.commit_span(committed).expect("commits");
        (stream, generation, hypothesis, committed)
    }

    fn propose(
        tool_id: ToolId,
        generation_id: GenerationId,
        basis_span_id: SpanId,
    ) -> ProposeToolCall {
        ProposeToolCall {
            tool_id,
            generation_id,
            basis_span_id,
            arguments: json!({"day": "monday"}),
        }
    }

    #[test]
    fn identities_count_in_the_tools_space_and_a_refusal_takes_none() {
        let (stream, generation, hypothesis, committed) = stream();
        let mut domain = ToolsDomain::new();
        let tool = declare(&mut domain, SideEffectClass::IdempotentWrite);
        assert_eq!(tool.to_string(), "00000004-0000-4000-8000-000000000001");

        let barred = domain.propose_tool_call(&stream, propose(tool, generation, hypothesis));
        assert_eq!(
            barred,
            Err(ProposeToolCallError::Barred(ProvisionalInputBarrier {
                side_effect: SideEffectClass::IdempotentWrite,
            }))
        );
        assert!(domain.calls().is_empty());
        assert_eq!(domain.events().len(), 1, "a refusal records no event");

        let proposed = domain
            .propose_tool_call(&stream, propose(tool, generation, committed))
            .expect("a committed span admits it");
        assert_eq!(
            proposed.call_id.to_string(),
            "00000004-0000-4000-8000-000000000002",
            "the barred proposal took no identity"
        );
        let row = &domain.tool_calls_view()[0];
        assert_eq!(row.state, ToolCallState::Proposed);
        assert_eq!(row.side_effect, SideEffectClass::IdempotentWrite);
        assert_eq!(row.basis_span_id, committed);
        assert_eq!(row.arguments, json!({"day": "monday"}));
    }

    #[test]
    fn a_proposal_refuses_an_unknown_tool_then_generation_then_span_before_any_state() {
        let (mut stream, generation, hypothesis, _) = stream();
        stream.cancel_generation(generation).expect("cancels");
        let mut domain = ToolsDomain::new();
        let tool = declare(&mut domain, SideEffectClass::NonIdempotentWrite);
        let unknown_tool = ToolId::from(Uuid::from_u128(9));
        let unknown_generation = GenerationId::from(Uuid::from_u128(9));
        let unknown_span = SpanId::from(Uuid::from_u128(9));

        let answer = |domain: &mut ToolsDomain, input: ProposeToolCall| {
            let speculative = ProposeSpeculativeToolCall {
                tool_id: input.tool_id,
                generation_id: input.generation_id,
                basis_span_id: input.basis_span_id,
                arguments: input.arguments.clone(),
            };
            (
                domain.propose_tool_call(&stream, input),
                domain.propose_speculative_tool_call(&stream, speculative),
            )
        };
        assert_eq!(
            answer(
                &mut domain,
                propose(unknown_tool, unknown_generation, unknown_span)
            ),
            (
                Err(ProposeToolCallError::UnknownTool(ToolNotFound {
                    tool_id: unknown_tool
                })),
                Err(ProposeSpeculativeToolCallError::UnknownTool(ToolNotFound {
                    tool_id: unknown_tool
                })),
            )
        );
        let not_found = GenerationNotFound {
            generation_id: unknown_generation,
        };
        assert_eq!(
            answer(&mut domain, propose(tool, unknown_generation, unknown_span)),
            (
                Err(ProposeToolCallError::UnknownGeneration(not_found)),
                Err(ProposeSpeculativeToolCallError::UnknownGeneration(
                    not_found
                )),
            )
        );
        let not_found = SpanNotFound {
            span_id: unknown_span,
        };
        assert_eq!(
            answer(&mut domain, propose(tool, generation, unknown_span)),
            (
                Err(ProposeToolCallError::UnknownSpan(not_found)),
                Err(ProposeSpeculativeToolCallError::UnknownSpan(not_found)),
            )
        );
        let stale = GenerationStateConflict {
            current: GenerationState::Cancelled,
        };
        assert_eq!(
            answer(&mut domain, propose(tool, generation, hypothesis)),
            (
                Err(ProposeToolCallError::StaleGeneration(stale)),
                Err(ProposeSpeculativeToolCallError::StaleGeneration(stale)),
            ),
            "a stale generation is refused before the barrier"
        );
        assert!(domain.calls().is_empty());
    }

    #[test]
    fn the_lifecycle_runs_through_the_store_and_a_refused_move_records_nothing() {
        let (stream, generation, hypothesis, _) = stream();
        let mut domain = ToolsDomain::new();
        let tool = declare(&mut domain, SideEffectClass::ReadOnly);
        let call_id = domain
            .propose_speculative_tool_call(
                &stream,
                ProposeSpeculativeToolCall {
                    tool_id: tool,
                    generation_id: generation,
                    basis_span_id: hypothesis,
                    arguments: json!([]),
                },
            )
            .expect("a read-only tool may act on a hypothesis")
            .call_id;
        let authorize = |confirmed| AuthorizeToolCall {
            call_id,
            confirmation_required: true,
            confirmed,
        };

        assert_eq!(
            domain.start_tool_call(call_id),
            Err(ToolCallMoveError::Conflict(ToolCallStateConflict {
                current: ToolCallState::Proposed,
            }))
        );
        assert_eq!(
            domain.authorize_tool_call(authorize(false)),
            Err(AuthorizeToolCallError::Unconfirmed(ConfirmationRequired {
                call_id
            }))
        );
        let recorded = domain.events().len();
        assert_eq!(
            domain.calls().get(call_id).map(|call| call.state()),
            Some(ToolCallState::Proposed)
        );

        domain
            .authorize_tool_call(authorize(true))
            .expect("authorizes");
        domain.start_tool_call(call_id).expect("starts");
        let result = domain
            .complete_tool_call(CompleteToolCall {
                call_id,
                result: json!({"seat": 4}),
            })
            .expect("completes");
        assert_eq!(result.result, json!({"seat": 4}));
        assert_eq!(domain.events().len(), recorded + 3);
        assert_eq!(
            domain.fail_tool_call(FailToolCall {
                call_id,
                failure: ToolFailure {
                    kind: ToolFailureKind::Cancelled,
                    detail: "late".to_owned(),
                },
            }),
            Err(ToolCallMoveError::Conflict(ToolCallStateConflict {
                current: ToolCallState::Completed,
            })),
            "a completed write is not undone"
        );
        assert_eq!(
            domain.reject_tool_call(RejectToolCall {
                call_id,
                reason: RejectionReason::Superseded,
            }),
            Err(ToolCallMoveError::Conflict(ToolCallStateConflict {
                current: ToolCallState::Completed,
            }))
        );
        assert_eq!(domain.events().len(), recorded + 3);
        assert!(matches!(
            domain.events().last(),
            Some(ToolsEvent::ToolCallResult(_))
        ));
    }

    #[test]
    fn a_move_of_an_unknown_call_is_refused_but_the_hook_answers_first() {
        let mut domain = ToolsDomain::new();
        let call_id = ToolCallId::from(Uuid::from_u128(9));
        assert_eq!(
            domain.authorize_tool_call(AuthorizeToolCall {
                call_id,
                confirmation_required: true,
                confirmed: false,
            }),
            Err(AuthorizeToolCallError::Unconfirmed(ConfirmationRequired {
                call_id
            }))
        );
        assert_eq!(
            domain.authorize_tool_call(AuthorizeToolCall {
                call_id,
                confirmation_required: false,
                confirmed: false,
            }),
            Err(AuthorizeToolCallError::Unknown { call_id })
        );
        assert_eq!(
            domain.start_tool_call(call_id),
            Err(ToolCallMoveError::Unknown { call_id })
        );
        assert!(domain.events().is_empty());
    }
}
