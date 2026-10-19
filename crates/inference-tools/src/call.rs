//! The `inference.tools.ToolCall` state machine: Proposed, then Authorized or Rejected, then
//! Started, then Completed or Failed.

use std::error::Error;
use std::fmt;

use serde::Serialize;

use inference_core::generation::{Generation, GenerationState, GenerationStateConflict};
use inference_core::id::GenerationId;
use inference_text::{Span, SpanId};

use crate::{
    AuthorizeToolCall, ConfirmationRequired, Json, ProvisionalInputBarrier, RejectionReason,
    SideEffectClass, Tool, ToolCallAuthorized, ToolCallFailed, ToolCallId, ToolCallProposed,
    ToolCallRejected, ToolCallResult, ToolCallStarted, ToolCallState, ToolCallStateConflict,
    ToolFailure, ToolId, committed,
};

/// Why `inference.tools.ProposeToolCall` proposed nothing, in the order the specification
/// decides it once the tool, generation and span exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposeError {
    /// The `stale-generation` outcome: the generation is not Active.
    StaleGeneration(GenerationStateConflict),
    /// The `barred` outcome: the span the proposal rests on is still a Hypothesis.
    Barred(ProvisionalInputBarrier),
}

impl fmt::Display for ProposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleGeneration(error) => fmt::Display::fmt(error, f),
            Self::Barred(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ProposeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::StaleGeneration(error) => Some(error),
            Self::Barred(error) => Some(error),
        }
    }
}

/// Why `inference.tools.ProposeSpeculativeToolCall` proposed nothing, in the order the
/// specification decides it once the tool, generation and span exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposeSpeculativeError {
    /// The `stale-generation` outcome: the generation is not Active.
    StaleGeneration(GenerationStateConflict),
    /// The `side-effecting` outcome: the tool is not read-only.
    SideEffecting(ProvisionalInputBarrier),
}

impl fmt::Display for ProposeSpeculativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleGeneration(error) => fmt::Display::fmt(error, f),
            Self::SideEffecting(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ProposeSpeculativeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::StaleGeneration(error) => Some(error),
            Self::SideEffecting(error) => Some(error),
        }
    }
}

/// Why `inference.tools.AuthorizeToolCall` moved nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizeError {
    /// The `unconfirmed` outcome: confirmation is required and none was given.
    Unconfirmed(ConfirmationRequired),
    /// The `wrong-state` outcome: the call is not Proposed.
    Conflict(ToolCallStateConflict),
}

impl fmt::Display for AuthorizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfirmed(error) => fmt::Display::fmt(error, f),
            Self::Conflict(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for AuthorizeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unconfirmed(error) => Some(error),
            Self::Conflict(error) => Some(error),
        }
    }
}

/// `inference.tools.ToolCall`: a tool proposal a generation made, correlated to it. It references
/// its tool by `tool_id`, its generation by `generation_id` and the inbound span it rests on by
/// `basis_span_id`, and carries its tool's side-effect class, copied at proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolCall {
    call_id: ToolCallId,
    tool_id: ToolId,
    generation_id: GenerationId,
    side_effect: SideEffectClass,
    basis_span_id: SpanId,
    arguments: Json,
    state: ToolCallState,
}

impl ToolCall {
    /// Runs `inference.tools.ProposeToolCall`, the default path, for `tool`, proposed by
    /// `generation` and resting on `basis`: generates a [`ToolCallId`] and returns the Proposed
    /// call, carrying `tool`'s class, and its [`ToolCallProposed`].
    ///
    /// # Errors
    ///
    /// As [`ToolCall::propose_as`].
    pub fn propose(
        tool: &Tool,
        generation: &Generation,
        basis: &Span,
        arguments: Json,
    ) -> Result<(Self, ToolCallProposed), ProposeError> {
        Self::propose_as(ToolCallId::generate(), tool, generation, basis, arguments)
    }

    /// Runs `inference.tools.ProposeToolCall` under `call_id` instead of a generated one, and
    /// otherwise as [`ToolCall::propose`]. The barrier reads the held state of `basis`, never a
    /// caller's claim, and refuses a Hypothesis for every class: speculation on a hypothesis is
    /// [`ToolCall::propose_speculative_as`].
    ///
    /// # Errors
    ///
    /// [`ProposeError::StaleGeneration`] carrying the generation's state when it is not Active,
    /// then [`ProposeError::Barred`] carrying `tool`'s class when `basis` is not Committed. No
    /// call is created and `call_id` is not used.
    pub fn propose_as(
        call_id: ToolCallId,
        tool: &Tool,
        generation: &Generation,
        basis: &Span,
        arguments: Json,
    ) -> Result<(Self, ToolCallProposed), ProposeError> {
        active(generation).map_err(ProposeError::StaleGeneration)?;
        if !committed(basis.state()) {
            return Err(ProposeError::Barred(ProvisionalInputBarrier {
                side_effect: tool.side_effect(),
            }));
        }
        Ok(Self::proposed(
            call_id,
            tool,
            generation,
            basis,
            arguments,
            tool.side_effect(),
        ))
    }

    /// Runs `inference.tools.ProposeSpeculativeToolCall`, the explicit speculative path: generates
    /// a [`ToolCallId`] and otherwise as [`ToolCall::propose_speculative_as`].
    ///
    /// # Errors
    ///
    /// As [`ToolCall::propose_speculative_as`].
    pub fn propose_speculative(
        tool: &Tool,
        generation: &Generation,
        basis: &Span,
        arguments: Json,
    ) -> Result<(Self, ToolCallProposed), ProposeSpeculativeError> {
        Self::propose_speculative_as(ToolCallId::generate(), tool, generation, basis, arguments)
    }

    /// Runs `inference.tools.ProposeSpeculativeToolCall` under `call_id`: a read-only tool may act
    /// on a hypothesis, so the state of `basis` is not read; the call is read-only.
    ///
    /// # Errors
    ///
    /// [`ProposeSpeculativeError::StaleGeneration`] carrying the generation's state when it is
    /// not Active, then [`ProposeSpeculativeError::SideEffecting`] carrying `tool`'s class when it
    /// is not read-only. No call is created and `call_id` is not used.
    pub fn propose_speculative_as(
        call_id: ToolCallId,
        tool: &Tool,
        generation: &Generation,
        basis: &Span,
        arguments: Json,
    ) -> Result<(Self, ToolCallProposed), ProposeSpeculativeError> {
        active(generation).map_err(ProposeSpeculativeError::StaleGeneration)?;
        if tool.side_effect() != SideEffectClass::ReadOnly {
            return Err(ProposeSpeculativeError::SideEffecting(
                ProvisionalInputBarrier {
                    side_effect: tool.side_effect(),
                },
            ));
        }
        Ok(Self::proposed(
            call_id,
            tool,
            generation,
            basis,
            arguments,
            SideEffectClass::ReadOnly,
        ))
    }

    fn proposed(
        call_id: ToolCallId,
        tool: &Tool,
        generation: &Generation,
        basis: &Span,
        arguments: Json,
        side_effect: SideEffectClass,
    ) -> (Self, ToolCallProposed) {
        let call = Self {
            call_id,
            tool_id: tool.tool_id(),
            generation_id: generation.generation_id(),
            side_effect,
            basis_span_id: basis.span_id(),
            arguments,
            state: ToolCallState::Proposed,
        };
        let event = ToolCallProposed {
            call_id,
            tool_id: call.tool_id,
            generation_id: call.generation_id,
        };
        (call, event)
    }

    /// Runs `inference.tools.AuthorizeToolCall`: Proposed to Authorized, once the confirmation
    /// hook ([`AuthorizeToolCall::confirmation`]) admits it. `input.call_id` is not compared with
    /// the call's; the store that looked the call up by it has.
    ///
    /// # Errors
    ///
    /// [`AuthorizeError::Unconfirmed`] when confirmation is required and none was given, then
    /// [`AuthorizeError::Conflict`] carrying the held state when the call is not Proposed.
    pub fn authorize(
        &self,
        input: &AuthorizeToolCall,
    ) -> Result<(Self, ToolCallAuthorized), AuthorizeError> {
        input.confirmation().map_err(AuthorizeError::Unconfirmed)?;
        let call = self
            .moved(&[ToolCallState::Proposed], ToolCallState::Authorized)
            .map_err(AuthorizeError::Conflict)?;
        Ok((
            call,
            ToolCallAuthorized {
                call_id: self.call_id,
            },
        ))
    }

    /// Runs `inference.tools.RejectToolCall`: Proposed or Authorized to Rejected.
    ///
    /// # Errors
    ///
    /// [`ToolCallStateConflict`] carrying the held state otherwise.
    pub fn reject(
        &self,
        reason: RejectionReason,
    ) -> Result<(Self, ToolCallRejected), ToolCallStateConflict> {
        let call = self.moved(
            &[ToolCallState::Proposed, ToolCallState::Authorized],
            ToolCallState::Rejected,
        )?;
        Ok((
            call,
            ToolCallRejected {
                call_id: self.call_id,
                reason,
            },
        ))
    }

    /// Runs `inference.tools.StartToolCall`: Authorized to Started.
    ///
    /// # Errors
    ///
    /// [`ToolCallStateConflict`] carrying the held state when the call is not Authorized.
    pub fn start(&self) -> Result<(Self, ToolCallStarted), ToolCallStateConflict> {
        let call = self.moved(&[ToolCallState::Authorized], ToolCallState::Started)?;
        Ok((
            call,
            ToolCallStarted {
                call_id: self.call_id,
            },
        ))
    }

    /// Runs `inference.tools.CompleteToolCall`: Started to Completed, with `result`.
    ///
    /// # Errors
    ///
    /// [`ToolCallStateConflict`] carrying the held state when the call is not Started.
    pub fn complete(&self, result: Json) -> Result<(Self, ToolCallResult), ToolCallStateConflict> {
        let call = self.moved(&[ToolCallState::Started], ToolCallState::Completed)?;
        Ok((
            call,
            ToolCallResult {
                call_id: self.call_id,
                result,
            },
        ))
    }

    /// Runs `inference.tools.FailToolCall`: Started to Failed, with `failure`. No transition
    /// leaves Failed: a retry of the tool is the executor's, inside Started.
    ///
    /// # Errors
    ///
    /// [`ToolCallStateConflict`] carrying the held state when the call is not Started.
    pub fn fail(
        &self,
        failure: ToolFailure,
    ) -> Result<(Self, ToolCallFailed), ToolCallStateConflict> {
        let call = self.moved(&[ToolCallState::Started], ToolCallState::Failed)?;
        Ok((
            call,
            ToolCallFailed {
                call_id: self.call_id,
                failure,
            },
        ))
    }

    /// The call in `to`, when it holds one of `from`.
    fn moved(
        &self,
        from: &[ToolCallState],
        to: ToolCallState,
    ) -> Result<Self, ToolCallStateConflict> {
        if !from.contains(&self.state) {
            return Err(ToolCallStateConflict {
                current: self.state,
            });
        }
        Ok(Self {
            state: to,
            ..self.clone()
        })
    }

    #[must_use]
    pub const fn call_id(&self) -> ToolCallId {
        self.call_id
    }

    #[must_use]
    pub const fn tool_id(&self) -> ToolId {
        self.tool_id
    }

    #[must_use]
    pub const fn generation_id(&self) -> GenerationId {
        self.generation_id
    }

    #[must_use]
    pub const fn side_effect(&self) -> SideEffectClass {
        self.side_effect
    }

    #[must_use]
    pub const fn basis_span_id(&self) -> SpanId {
        self.basis_span_id
    }

    #[must_use]
    pub const fn arguments(&self) -> &Json {
        &self.arguments
    }

    #[must_use]
    pub const fn state(&self) -> ToolCallState {
        self.state
    }
}

/// Refuses a generation that is not Active with the state it holds.
fn active(generation: &Generation) -> Result<(), GenerationStateConflict> {
    if generation.state() == GenerationState::Active {
        Ok(())
    } else {
        Err(GenerationStateConflict {
            current: generation.state(),
        })
    }
}
