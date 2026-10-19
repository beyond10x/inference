//! Tool proposals of a generation, behind the `tools` feature (`docs/design/inference.md` 237).
//!
//! A model that claims `inference.model.Capability::ToolCalling` may end a model request with a
//! structured [`ToolProposal`] instead of, or after, its prose. The proposal never reaches the
//! caller as a [`TextDelta`](inference_text::TextDelta): it is taken from the handle through
//! [`ToolCalling::take_proposal`], so structured proposals stay distinguishable from user-visible
//! prose (374). The caller turns it into an `inference.tools.ProposeToolCall` of the generation
//! that made it.
//!
//! What became of the proposal returns to the same generation as a [`ToolOutcome`] through
//! [`ToolCalling::integrate`], which issues the generation's next model request with the outcome
//! appended. An ordinary text model integrates a tool result at a safe boundary only, the end of
//! the model request that emitted the proposal (402, 721): a proposal is taken once that request's
//! text is delivered, and the outcome is integrated as a request of its own. The generation stays
//! Active from the proposal to its completion, so every segment emitted after the outcome carries
//! the generation's identity (376).

use inference_core::generation::GenerationStateConflict;
use inference_tools::{Json, RejectionReason, ToolFailure, ToolId};

use crate::GenerationHandle;

/// A structured tool proposal a model request ended with: the tool and the arguments the model
/// proposes to call it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolProposal {
    pub tool_id: ToolId,
    pub arguments: Json,
}

/// What became of a proposal, as the generation that made it is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutcome {
    /// The call completed with this result (`inference.tools.ToolCallResult`).
    Result(Json),
    /// The call started and failed (`inference.tools.ToolCallFailed`).
    Failed(ToolFailure),
    /// The call was rejected before it started (`inference.tools.ToolCallRejected`).
    Rejected(RejectionReason),
}

/// A running generation whose model proposes tool calls; see the module documentation.
pub trait ToolCalling: GenerationHandle {
    /// Takes the proposal the generation's latest model request ended with, once that request's
    /// text has been delivered; none while text of it is still to be delivered, when it ended in
    /// prose only, or when its proposal was taken already.
    fn take_proposal(&mut self) -> Option<ToolProposal>;

    /// Issues the generation's next model request with `outcome` appended; its text and any
    /// proposal it ends with are delivered as for any request.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the generation's state when it is not Active: the
    /// outcome rejoins the generation that proposed and no other, and nothing is issued.
    fn integrate(&mut self, outcome: ToolOutcome) -> Result<(), GenerationStateConflict>;
}
