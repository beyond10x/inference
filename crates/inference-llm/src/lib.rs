//! The LLM contract of the inference framework, over the `llm` model contract.
//!
//! A text model is an [`llm_core::Model`]: its request, conversation items, tool calls, stream
//! events, usage and failures are `llm`'s types, re-exported here as [`llm_core`]. This crate
//! defines no model of its own. It adds what a graph node needs on top: the identity of a
//! generation (run, producing node, epoch), the generation's events as `ess/domains/stream.yaml`
//! declares them, the commit point of each segment, input offered while a generation runs, and the
//! maximum concurrency the adapter record declares.
//!
//! A [`Generator`] pairs an `inference.model.Adapter` record with the [`llm_core::Model`] that
//! implements it. [`Generator::start`] begins one generation for a [`GenerationRequest`], whose
//! `turn` is the `llm` request the generation's turns build on, and refuses a start past the
//! adapter's maximum concurrency with [`ConcurrencyLimitReached`]; a generation runs until its
//! [`GenerationHandle`] is dropped. The handle yields the generation's [`GenerationEvent`]s, the
//! outbound commands of `ess/domains/stream.yaml`: `StartGeneration`, then `EmitTextDelta` and
//! `CommitSegment` for each segment, then `CompleteGeneration`, or `CancelGeneration` when a turn
//! fails. `CancelGeneration`, `SupersedeGeneration` and `CompleteGeneration` are also control,
//! called on the handle, and a cancel reaches the running turn through `llm`'s
//! [`llm_core::Cancel`].
//!
//! A generation is a conversation of turns, and no turn asks an empty request. The first
//! [`llm_core::Model::turn`] runs on the request as it was given when it carries items, and
//! otherwise on the first input; each later one runs on the conversation so far, the items every
//! earlier turn returned included, once there is something new to answer: input offered while it
//! runs, as user text, or the results of every tool call the latest turn ended with. A turn that
//! fails cancels the generation: [`GenerationEvent::Cancelled`] is delivered with the segments it
//! discarded, and [`GenerationHandle::failure`] keeps the `llm` error. The wrapper makes one
//! attempt per turn, as [`llm_core::Model::turn`] runs one: a caller that wants a failure before
//! output retried wraps its model in `llm`'s routing, its fallback run, before handing it to a
//! [`Generator`]. The text of a turn is the generation's output, under `llm`'s rule that every
//! [`StreamEvent`] is final, that each assistant message of a successful turn carries exactly the
//! text deltas shown for it, and that a message no delta was shown for carries the text the
//! server sent. So the commit point is the delta: each becomes one segment, committed as it is
//! emitted; a turn that showed no delta gives the assistant text of its outcome as one segment
//! when it ends.
//!
//! Output streaming, incremental input and input during output are distinct capabilities
//! (`inference.model.Capability`), and a persistent connection proves none of them. What the
//! contract can observe, it decides here:
//!
//! * [`Generator::new`] refuses an adapter record claiming what its model's
//!   [`llm_core::Capabilities`] do not declare, with [`CapabilityNotDeclared`]:
//!   [`Capability::OutputStreaming`] needs `output_streaming`, [`Capability::ToolCalling`] needs
//!   `tools`, and [`Capability::NativeDuplex`] needs `input_during_output`.
//!   [`Capability::ConcurrentInputOutput`] needs no declaration: the wrapper itself takes input
//!   while the generation emits output, as the next turn's text.
//! * An adapter claiming [`Capability::OutputStreaming`] delivers each delta of a turn as a
//!   [`TextDelta`] while the generation is still Active, and the turn may still be running. One
//!   that does not delivers each turn's text as one segment, held until the generation completes
//!   or until a turn ends with tool calls, the safe boundary at which the caller must see them;
//!   both emit every segment into the Active generation, as `EmitTextDelta` requires.
//! * Input offered while a generation is Active is admitted by [`GenerationOutput::admit_input`]
//!   only when the adapter claims [`Capability::ConcurrentInputOutput`], and refused otherwise
//!   with [`CapabilityNotClaimed`] naming it. A handle takes input only as the [`AdmittedInput`]
//!   that admission returns, and the text inside is reachable only through
//!   [`GenerationOutput::release`] of the generation that admitted it, while that generation is
//!   still Active; input admitted elsewhere is refused with [`AdmittedByAnotherGeneration`]. So no
//!   input the adapter does not claim reaches the model, and an admission is taken once. Admitted
//!   input is the user text of the next turn, not of the running one: no `llm` protocol carries
//!   input during output.
//! * A turn ending with tool calls stages them as proposals, which never reach the caller as
//!   [`TextDelta`]s: structured proposals stay distinguishable from prose. The caller takes each
//!   with [`GenerationHandle::take_proposal`] once the turn's text is delivered and answers it
//!   with [`GenerationHandle::integrate`]; the next turn runs once every call of the round is
//!   answered, so every turn is a request `llm` validates. While a call is unanswered the
//!   generation waits for it and does not complete by itself; [`GenerationHandle::complete`]
//!   ends it regardless. The generation stays Active from the proposal to its completion, so
//!   every segment after a result carries its identity.
//!
//! [`GenerationOutput`] is the bookkeeping of one generation: the generation and the segments it
//! owns, moved only through the transitions of `inference-core` and `inference-text`. The crate
//! carries no concrete model, backend or provider; the scripted mock model is
//! `inference-testkit`'s, behind its `llm` feature. Behind the `tools` feature, which takes the
//! optional dependency on `inference-tools`, [`tools`] maps declared tools onto `llm`'s tool
//! specifications and the outcomes of the tools domain onto `llm`'s tool results.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::future::{self, Future};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::generation::{
    Generation, GenerationCancelled, GenerationCompleted, GenerationStarted, GenerationState,
    GenerationStateConflict, GenerationSuperseded, StartGeneration,
};
use inference_core::id::{GenerationId, NodeId, RunId};
use inference_core::model::{Adapter, Capability};
use inference_text::{
    OutputSegment, SegmentDiscarded, SegmentState, SegmentStateConflict, TextDelta,
    TextSegmentCommitted,
};
pub use llm_core;
use llm_core::{
    BoxFuture, CallId, Cancel, Item, Model, StreamEvent, StreamSink, ToolCall, TurnOutcome,
    TurnRequest,
};
use serde_json::Value;

/// Declared tools as `llm` tool specifications, and tool outcomes as `llm` tool results, behind
/// the `tools` feature.
#[cfg(feature = "tools")]
pub mod tools;

/// The revision of this contract, for the `contract_revision` an adapter implementing it
/// declares.
pub const CONTRACT_REVISION: &str = env!("CARGO_PKG_VERSION");

/// A request for one generation: the run, producing node and epoch it is started under, and the
/// `llm` request its first turn asks the model.
#[derive(Debug, Clone, PartialEq)]
pub struct GenerationRequest {
    pub run_id: RunId,
    pub node_id: NodeId,
    pub epoch: i64,
    pub turn: TurnRequest,
}

impl GenerationRequest {
    /// The `inference.stream.StartGeneration` input this request starts its generation with.
    #[must_use]
    pub const fn start_generation(&self) -> StartGeneration {
        StartGeneration {
            run_id: self.run_id,
            node_id: self.node_id,
            epoch: self.epoch,
        }
    }
}

/// An event of one generation, as a [`GenerationHandle`] yields it, in the order its commands ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerationEvent {
    /// `inference.stream.GenerationStarted`, always the first.
    Started(GenerationStarted),
    /// `inference.stream.TextDelta`: a Tentative segment was emitted.
    TextDelta(TextDelta),
    /// `inference.stream.TextSegmentCommitted`: a segment was committed.
    SegmentCommitted(TextSegmentCommitted),
    /// `inference.stream.GenerationCompleted`, always the last of a generation that completes.
    Completed(GenerationCompleted),
    /// `inference.stream.GenerationCancelled` of a generation whose turn failed, with the
    /// `SegmentDiscarded` of each segment the cancel discarded, in emission order; always the
    /// last. [`GenerationHandle::failure`] holds the `llm` error. A cancel the caller asks for
    /// returns its event instead.
    Cancelled(GenerationCancelled, Vec<SegmentDiscarded>),
}

/// The adapter does not claim `capability`, so the input offered while its generation is Active
/// is refused and nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityNotClaimed {
    pub capability: Capability,
}

impl fmt::Display for CapabilityNotClaimed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the adapter does not claim {:?}, so the input is refused",
            self.capability
        )
    }
}

impl Error for CapabilityNotClaimed {}

/// The adapter record claims `capability`, which the `llm` model implementing it does not
/// declare in its [`llm_core::Capabilities`], so no generator is made of the two:
/// [`Capability::OutputStreaming`] needs `output_streaming`, [`Capability::ToolCalling`] needs
/// `tools`, and [`Capability::NativeDuplex`] needs `input_during_output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityNotDeclared {
    pub capability: Capability,
}

impl fmt::Display for CapabilityNotDeclared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the adapter claims {:?}, which its llm model does not declare",
            self.capability
        )
    }
}

impl Error for CapabilityNotDeclared {}

/// Why input offered to a generation was refused; nothing changed either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputRefused {
    /// The generation is Active and the adapter does not claim
    /// [`Capability::ConcurrentInputOutput`].
    CapabilityNotClaimed(CapabilityNotClaimed),
    /// The generation is not Active, so no output is running to take input beside.
    GenerationStateConflict(GenerationStateConflict),
    /// The [`AdmittedInput`] was admitted by another generation than the one it was handed to.
    AdmittedByAnotherGeneration(AdmittedByAnotherGeneration),
}

impl fmt::Display for InputRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapabilityNotClaimed(error) => fmt::Display::fmt(error, f),
            Self::GenerationStateConflict(error) => fmt::Display::fmt(error, f),
            Self::AdmittedByAnotherGeneration(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for InputRefused {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CapabilityNotClaimed(error) => Some(error),
            Self::GenerationStateConflict(error) => Some(error),
            Self::AdmittedByAnotherGeneration(error) => Some(error),
        }
    }
}

/// Input admitted by the generation `admitted_by` was handed to the generation `offered_to`, so
/// it is refused and nothing changed: admission holds for the generation that granted it only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmittedByAnotherGeneration {
    pub admitted_by: GenerationId,
    pub offered_to: GenerationId,
}

impl fmt::Display for AdmittedByAnotherGeneration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the input was admitted by generation {} and is refused by generation {}",
            self.admitted_by, self.offered_to
        )
    }
}

impl Error for AdmittedByAnotherGeneration {}

/// The generator already runs `max_concurrency` generations, the maximum concurrency its adapter
/// declares, so a further start is refused and nothing started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConcurrencyLimitReached {
    pub max_concurrency: i64,
}

impl fmt::Display for ConcurrencyLimitReached {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the generator already runs {} generation(s), its declared maximum concurrency",
            self.max_concurrency
        )
    }
}

impl Error for ConcurrencyLimitReached {}

impl From<AdmittedByAnotherGeneration> for InputRefused {
    fn from(error: AdmittedByAnotherGeneration) -> Self {
        Self::AdmittedByAnotherGeneration(error)
    }
}

impl From<CapabilityNotClaimed> for InputRefused {
    fn from(error: CapabilityNotClaimed) -> Self {
        Self::CapabilityNotClaimed(error)
    }
}

impl From<GenerationStateConflict> for InputRefused {
    fn from(error: GenerationStateConflict) -> Self {
        Self::GenerationStateConflict(error)
    }
}

/// Input [`GenerationOutput::admit_input`] admitted into an Active generation whose adapter
/// claims [`Capability::ConcurrentInputOutput`]. Only admission constructs it, and only
/// [`GenerationOutput::release`] of the generation that admitted it, while that generation is
/// still Active, gives its text up. It is neither `Clone` nor `Copy`, and release consumes it, so
/// one admission is taken once; its `Debug` form shows the admitting generation, not the text, and
/// it is not `PartialEq`, so comparing two admissions confirms no guessed text.
///
/// ```compile_fail
/// fn cloneable<T: Clone>() {}
/// cloneable::<inference_llm::AdmittedInput>();
/// ```
///
/// ```compile_fail
/// fn comparable<T: PartialEq>() {}
/// comparable::<inference_llm::AdmittedInput>();
/// ```
pub struct AdmittedInput {
    generation_id: GenerationId,
    text: String,
}

impl fmt::Debug for AdmittedInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdmittedInput")
            .field("generation_id", &self.generation_id)
            .finish_non_exhaustive()
    }
}

impl AdmittedInput {
    /// The generation that admitted the input.
    #[must_use]
    pub const fn generation_id(&self) -> GenerationId {
        self.generation_id
    }
}

/// One generation and the segments it owns, with the capabilities of the adapter producing it.
/// Every move runs a transition of [`Generation`] or [`OutputSegment`], so a refused move changes
/// nothing; a segment's transition consumes it and the moved segment takes its place.
/// Superseding or cancelling the generation discards every segment it owns in the same call, as
/// the store of both does.
///
/// It is not `Clone`: a copy taken while the generation is Active would keep that state after
/// the generation moved on, and release input to a generation that is stale.
///
/// ```compile_fail
/// fn cloneable<T: Clone>() {}
/// cloneable::<inference_llm::GenerationOutput>();
/// ```
#[derive(Debug, PartialEq, Eq)]
pub struct GenerationOutput {
    capabilities: Vec<Capability>,
    generation: Generation,
    segments: Vec<OutputSegment>,
}

impl GenerationOutput {
    /// Starts the generation `request` asks for on behalf of `adapter`, whose claims it keeps,
    /// and returns it with its [`GenerationStarted`].
    #[must_use]
    pub fn start(adapter: &Adapter, request: &GenerationRequest) -> (Self, GenerationStarted) {
        let (generation, event) = Generation::start(request.start_generation());
        let output = Self {
            capabilities: adapter.capabilities().to_vec(),
            generation,
            segments: Vec::new(),
        };
        (output, event)
    }

    /// Runs `inference.stream.EmitTextDelta`: adds a Tentative segment carrying `text`, numbered
    /// one past the last segment, the first one 1.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the generation's state when it is not Active; no
    /// segment is added.
    pub fn emit(&mut self, text: String) -> Result<TextDelta, GenerationStateConflict> {
        let sequence = i64::try_from(self.segments.len()).map_or(i64::MAX, |count| count + 1);
        let (segment, event) = OutputSegment::emit(&self.generation, sequence, text)?;
        self.segments.push(segment);
        Ok(event)
    }

    /// Runs `inference.stream.CommitSegment` on every Tentative segment, in the order they were
    /// emitted, and returns their [`TextSegmentCommitted`] events. As `ess/domains/stream.yaml`
    /// declares it, `CommitSegment` has no guard on the generation's state: the Tentative segment
    /// of a Completed generation commits. Superseding or cancelling discards every segment, so
    /// none of a stale generation is left Tentative to commit.
    ///
    /// # Errors
    ///
    /// [`SegmentStateConflict`], the only refusal of `CommitSegment`, carrying the state of a
    /// segment that is not Tentative. Only Tentative segments are selected, so it is not reached.
    pub fn commit_tentative(&mut self) -> Result<Vec<TextSegmentCommitted>, SegmentStateConflict> {
        let mut events = Vec::new();
        let mut refusal = None;
        self.segments = std::mem::take(&mut self.segments)
            .into_iter()
            .map(|segment| {
                if refusal.is_some() || segment.state() != SegmentState::Tentative {
                    return segment;
                }
                match segment.commit() {
                    Ok((committed, event)) => {
                        events.push(event);
                        committed
                    }
                    Err(refused) => {
                        refusal = Some(refused.conflict());
                        refused.into_segment()
                    }
                }
            })
            .collect();
        refusal.map_or(Ok(events), Err)
    }

    /// Runs `inference.stream.CompleteGeneration`.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the held state when the generation is not Active.
    pub fn complete(&mut self) -> Result<GenerationCompleted, GenerationStateConflict> {
        let (generation, event) = self.generation.complete()?;
        self.generation = generation;
        Ok(event)
    }

    /// Runs `inference.stream.SupersedeGeneration` and discards every segment of the generation
    /// not yet Discarded or Played, returning the [`SegmentDiscarded`] of each in emission order.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the held state when the generation is not Active;
    /// no segment is discarded.
    pub fn supersede(
        &mut self,
    ) -> Result<(GenerationSuperseded, Vec<SegmentDiscarded>), GenerationStateConflict> {
        let (generation, event) = self.generation.supersede()?;
        self.generation = generation;
        Ok((event, self.discard_segments()))
    }

    /// Runs `inference.stream.CancelGeneration` and discards every segment of the generation not
    /// yet Discarded or Played, returning the [`SegmentDiscarded`] of each in emission order.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the held state when the generation is not Active;
    /// no segment is discarded.
    pub fn cancel(
        &mut self,
    ) -> Result<(GenerationCancelled, Vec<SegmentDiscarded>), GenerationStateConflict> {
        let (generation, event) = self.generation.cancel()?;
        self.generation = generation;
        Ok((event, self.discard_segments()))
    }

    /// Admits `text` offered while the generation runs.
    ///
    /// # Errors
    ///
    /// [`InputRefused::GenerationStateConflict`] when the generation is not Active, and
    /// otherwise [`InputRefused::CapabilityNotClaimed`] naming
    /// [`Capability::ConcurrentInputOutput`] when the adapter does not claim it.
    pub fn admit_input(&self, text: String) -> Result<AdmittedInput, InputRefused> {
        self.require_active()?;
        if !self
            .capabilities
            .contains(&Capability::ConcurrentInputOutput)
        {
            return Err(CapabilityNotClaimed {
                capability: Capability::ConcurrentInputOutput,
            }
            .into());
        }
        Ok(AdmittedInput {
            generation_id: self.generation.generation_id(),
            text,
        })
    }

    /// Releases the text of `input` to the implementation producing this generation, consuming
    /// the admission. It is the only way to the text of an [`AdmittedInput`].
    ///
    /// # Errors
    ///
    /// [`InputRefused::AdmittedByAnotherGeneration`] when another generation admitted `input`,
    /// and otherwise [`InputRefused::GenerationStateConflict`] when this generation is no longer
    /// Active; nothing is released.
    pub fn release(&self, input: AdmittedInput) -> Result<String, InputRefused> {
        let offered_to = self.generation.generation_id();
        if input.generation_id != offered_to {
            return Err(AdmittedByAnotherGeneration {
                admitted_by: input.generation_id,
                offered_to,
            }
            .into());
        }
        self.require_active()?;
        Ok(input.text)
    }

    #[must_use]
    pub const fn generation(&self) -> &Generation {
        &self.generation
    }

    /// The segments the generation owns, in emission order.
    #[must_use]
    pub fn segments(&self) -> &[OutputSegment] {
        &self.segments
    }

    /// The capabilities the producing adapter claimed when the generation started.
    #[must_use]
    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    fn require_active(&self) -> Result<(), GenerationStateConflict> {
        let current = self.generation.state();
        if current == GenerationState::Active {
            Ok(())
        } else {
            Err(GenerationStateConflict { current })
        }
    }

    fn discard_segments(&mut self) -> Vec<SegmentDiscarded> {
        let mut events = Vec::new();
        self.segments = std::mem::take(&mut self.segments)
            .into_iter()
            .map(|segment| match segment.discard() {
                Ok((discarded, event)) => {
                    events.push(event);
                    discarded
                }
                Err(refused) => refused.into_segment(),
            })
            .collect();
        events
    }
}

/// Why a tool result was not integrated into a generation; nothing changed either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeRefused {
    /// The generation is not Active: the result rejoins the generation that proposed and no
    /// other, and no turn runs.
    GenerationStateConflict(GenerationStateConflict),
    /// The call is not one of the latest turn that is still unanswered.
    NotAwaited(CallNotAwaited),
}

impl fmt::Display for OutcomeRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenerationStateConflict(error) => fmt::Display::fmt(error, f),
            Self::NotAwaited(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for OutcomeRefused {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::GenerationStateConflict(error) => Some(error),
            Self::NotAwaited(error) => Some(error),
        }
    }
}

impl From<GenerationStateConflict> for OutcomeRefused {
    fn from(error: GenerationStateConflict) -> Self {
        Self::GenerationStateConflict(error)
    }
}

impl From<CallNotAwaited> for OutcomeRefused {
    fn from(error: CallNotAwaited) -> Self {
        Self::NotAwaited(error)
    }
}

/// A result named `call_id`, which no unanswered call of the generation's latest turn carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallNotAwaited {
    pub call_id: CallId,
}

impl fmt::Display for CallNotAwaited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no unanswered call of the latest turn is {}, so its result is refused",
            self.call_id
        )
    }
}

impl Error for CallNotAwaited {}

/// The generations of one generator that run, shared by its clones, which are the same adapter.
#[derive(Debug, Clone, Default)]
struct Running(Arc<AtomicUsize>);

impl Running {
    /// Counts one more running generation, if fewer than `max_concurrency` run.
    fn admit(&self, max_concurrency: i64) -> Result<Slot, ConcurrencyLimitReached> {
        let limit = usize::try_from(max_concurrency).unwrap_or(usize::MAX);
        let slot = Slot(Arc::clone(&self.0));
        if self.0.fetch_add(1, Ordering::AcqRel) < limit {
            Ok(slot)
        } else {
            drop(slot);
            Err(ConcurrencyLimitReached { max_concurrency })
        }
    }
}

/// The place of one running generation among [`Running`], given back when it is dropped.
#[derive(Debug)]
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// An `inference.model.Adapter` of a text model: the adapter record it was declared with, the
/// capabilities it claims among it, and the [`llm_core::Model`] that implements it. A clone is
/// the same adapter: it shares the model and the count of running generations, so the maximum
/// concurrency holds across clones.
#[derive(Clone)]
pub struct Generator {
    adapter: Adapter,
    model: Arc<dyn Model>,
    running: Running,
}

impl fmt::Debug for Generator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Generator")
            .field("adapter", &self.adapter)
            .field("model", self.model.provenance())
            .finish_non_exhaustive()
    }
}

impl Generator {
    /// The adapter `adapter` implemented by `model`.
    ///
    /// # Errors
    ///
    /// [`CapabilityNotDeclared`] naming the first claim of `adapter` that `model` does not
    /// declare; no generator is made.
    pub fn new(adapter: Adapter, model: Arc<dyn Model>) -> Result<Self, CapabilityNotDeclared> {
        let declared = model.capabilities();
        let undeclared = adapter.capabilities().iter().find(|claim| match claim {
            Capability::OutputStreaming => !declared.output_streaming,
            Capability::NativeDuplex => !declared.input_during_output,
            Capability::ToolCalling => !declared.tools,
            _ => false,
        });
        if let Some(&capability) = undeclared {
            return Err(CapabilityNotDeclared { capability });
        }
        Ok(Self {
            adapter,
            model,
            running: Running::default(),
        })
    }

    /// The adapter record this generator was declared with, the capabilities it claims among it.
    #[must_use]
    pub const fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    /// The `llm` model that runs the generations' turns.
    #[must_use]
    pub fn model(&self) -> &dyn Model {
        self.model.as_ref()
    }

    /// Starts one generation for `request`; the handle yields its events, the first
    /// [`GenerationEvent::Started`]. The first turn runs once the handle is asked for the next
    /// event after that. A generation runs until its handle is dropped.
    ///
    /// # Errors
    ///
    /// [`ConcurrencyLimitReached`] carrying the adapter's `max_concurrency` when that many
    /// generations of this generator, or of a clone of it, already run; nothing starts.
    pub fn start(
        &mut self,
        request: GenerationRequest,
    ) -> Result<GenerationHandle, ConcurrencyLimitReached> {
        let slot = self.running.admit(self.adapter.max_concurrency())?;
        let (output, started) = GenerationOutput::start(&self.adapter, &request);
        Ok(GenerationHandle {
            model: Arc::clone(&self.model),
            streams: self.model.capabilities().output_streaming
                && output.capabilities().contains(&Capability::OutputStreaming),
            output,
            conversation: request.turn,
            first: true,
            inputs: VecDeque::new(),
            answered: false,
            awaited: Vec::new(),
            staged: VecDeque::new(),
            turn: None,
            cancel: Cancel::new(),
            waker: None,
            delivered: VecDeque::from([GenerationEvent::Started(started)]),
            held: VecDeque::new(),
            text: String::new(),
            shown: false,
            outcomes: Vec::new(),
            failure: None,
            _slot: slot,
        })
    }
}

/// The stream events of one running turn, as its sink accepted them and the handle has not yet
/// taken them.
type Accepted = Arc<Mutex<VecDeque<StreamEvent>>>;

/// The sink a turn streams into: it accepts every event, and the handle takes them at its next
/// poll.
struct Queue(Accepted);

impl StreamSink for Queue {
    fn emit(&mut self, event: StreamEvent) -> BoxFuture<'_, Result<(), llm_core::Error>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(event);
        Box::pin(future::ready(Ok(())))
    }
}

/// One turn of a generation, running.
struct Turn {
    future: BoxFuture<'static, Result<TurnOutcome, llm_core::Error>>,
    accepted: Accepted,
}

/// One running generation over an [`llm_core::Model`]; see the crate documentation.
///
/// It is polled for its events. [`GenerationHandle::poll_turn_event`] yields the events of the
/// turns that can run now and is ready with none once they are delivered and no turn runs: the
/// generation then awaits input or the results of its latest round, or has ended. It never
/// completes the generation. [`GenerationHandle::poll_event`] yields the same events and, asked
/// when no turn runs, none can start and no call is unanswered, completes the generation; while
/// calls of the latest turn are unanswered it is pending, and [`GenerationHandle::integrate`]
/// wakes it. [`GenerationHandle::complete`] completes the generation whatever is unanswered.
/// Iterating the handle blocks the calling thread until the next event of
/// [`GenerationHandle::poll_event`], and yields none while the generation awaits results, which
/// only the caller can give; it ends after [`GenerationEvent::Completed`] or
/// [`GenerationEvent::Cancelled`], or once the generation is superseded or cancelled.
///
/// It is not `Clone`: a copy taken while the generation is Active would deliver output for it
/// after it was superseded or cancelled.
///
/// ```compile_fail
/// fn cloneable<T: Clone>() {}
/// cloneable::<inference_llm::GenerationHandle>();
/// ```
pub struct GenerationHandle {
    model: Arc<dyn Model>,
    streams: bool,
    output: GenerationOutput,
    /// The conversation so far: the request's items and every item a turn returned or a result
    /// answered; the next turn asks it, with the inputs still waiting appended.
    conversation: TurnRequest,
    /// Whether no turn ran yet.
    first: bool,
    /// Input taken while the generation runs, the next turn's user text.
    inputs: VecDeque<String>,
    /// Whether a result was integrated since the latest turn ran.
    answered: bool,
    /// The calls of the latest turn whose results are still to come.
    awaited: Vec<CallId>,
    /// The calls of the latest turn not yet taken as proposals.
    staged: VecDeque<ToolCall>,
    turn: Option<Turn>,
    cancel: Cancel,
    /// The waker of the latest poll, woken by an integration and used to drive the running turn
    /// from [`GenerationHandle::take_proposal`].
    waker: Option<Waker>,
    delivered: VecDeque<GenerationEvent>,
    /// The events of an adapter that does not stream, held until the generation completes or a
    /// turn ends with calls.
    held: VecDeque<GenerationEvent>,
    /// The text of the running turn of an adapter that does not stream.
    text: String,
    /// Whether the running turn showed a text delta.
    shown: bool,
    outcomes: Vec<TurnOutcome>,
    failure: Option<llm_core::Error>,
    _slot: Slot,
}

impl fmt::Debug for GenerationHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenerationHandle")
            .field("output", &self.output)
            .field("conversation", &self.conversation)
            .field("inputs", &self.inputs)
            .field("awaited", &self.awaited)
            .field("staged", &self.staged)
            .field("running", &self.turn.is_some())
            .field("delivered", &self.delivered)
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl GenerationHandle {
    /// The generation and its segments, as they stand now.
    #[must_use]
    pub const fn output(&self) -> &GenerationOutput {
        &self.output
    }

    /// The conversation the next turn asks: the request's items and every item a turn returned
    /// or a result answered, in order.
    #[must_use]
    pub const fn conversation(&self) -> &TurnRequest {
        &self.conversation
    }

    /// The outcome of every turn that ended, in order: its stop reason, items and usage.
    #[must_use]
    pub fn outcomes(&self) -> &[TurnOutcome] {
        &self.outcomes
    }

    /// The `llm` failure of the turn that ended the generation, if one did; the generation was
    /// then cancelled, and [`GenerationEvent::Cancelled`] delivered.
    #[must_use]
    pub const fn failure(&self) -> Option<&llm_core::Error> {
        self.failure.as_ref()
    }

    /// Whether calls of the latest turn are still unanswered.
    #[must_use]
    pub fn awaits_results(&self) -> bool {
        !self.awaited.is_empty()
    }

    /// Polls for the next event of the turns that can run now: ready with it; pending while a
    /// turn runs and delivered nothing new; ready with none once every event is delivered and no
    /// turn runs or can start. It never completes the generation.
    pub fn poll_turn_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<GenerationEvent>> {
        self.waker = Some(cx.waker().clone());
        self.progress(cx);
        if let Some(event) = self.delivered.pop_front() {
            return Poll::Ready(Some(event));
        }
        if self.turn.is_some() {
            Poll::Pending
        } else {
            Poll::Ready(None)
        }
    }

    /// The next event of the turns that can run now, as [`GenerationHandle::poll_turn_event`]
    /// yields it: none once they are all delivered.
    pub fn next_turn_event(&mut self) -> impl Future<Output = Option<GenerationEvent>> + '_ {
        future::poll_fn(|cx| self.poll_turn_event(cx))
    }

    /// Polls for the generation's next event: as [`GenerationHandle::poll_turn_event`], and once
    /// that has none, pending while calls of the latest turn are unanswered, ready with none once
    /// the generation ended, and otherwise completing the generation and ready with
    /// [`GenerationEvent::Completed`] after anything an adapter that does not stream held.
    pub fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<GenerationEvent>> {
        match self.poll_turn_event(cx) {
            Poll::Ready(None) => {}
            polled => return polled,
        }
        if self.output.generation().state() != GenerationState::Active {
            return Poll::Ready(None);
        }
        if self.awaits_results() {
            return Poll::Pending;
        }
        self.finish();
        Poll::Ready(self.delivered.pop_front())
    }

    /// The generation's next event, as [`GenerationHandle::poll_event`] yields it.
    pub fn next_event(&mut self) -> impl Future<Output = Option<GenerationEvent>> + '_ {
        future::poll_fn(|cx| self.poll_event(cx))
    }

    /// Offers `text` to the running generation: admitted by [`GenerationOutput::admit_input`],
    /// then accepted as by [`GenerationHandle::accept_input`].
    ///
    /// # Errors
    ///
    /// The [`InputRefused`] of admission or acceptance; nothing is accepted.
    pub fn offer_input(&mut self, text: String) -> Result<(), InputRefused> {
        let input = self.output.admit_input(text)?;
        self.accept_input(input)
    }

    /// Takes input admitted into the running generation, through [`GenerationOutput::release`],
    /// as the user text of its next turn.
    ///
    /// # Errors
    ///
    /// The [`InputRefused`] of [`GenerationOutput::release`]: input admitted by another
    /// generation, or a generation no longer Active; nothing is accepted.
    pub fn accept_input(&mut self, input: AdmittedInput) -> Result<(), InputRefused> {
        let text = self.output.release(input)?;
        self.inputs.push_back(text);
        Ok(())
    }

    /// Runs `inference.stream.EmitTextDelta` for `text` produced outside the model's turns and
    /// queues the `TextDelta` for delivery; the segment stays Tentative.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the generation's state when it is not Active, the
    /// `stale-generation` outcome of `EmitTextDelta`: no segment is created and nothing is
    /// delivered.
    pub fn emit(&mut self, text: impl Into<String>) -> Result<TextDelta, GenerationStateConflict> {
        let delta = self.output.emit(text.into())?;
        self.delivered
            .push_back(GenerationEvent::TextDelta(delta.clone()));
        Ok(delta)
    }

    /// Takes the next tool call the latest turn ended with, once that turn ended and its text was
    /// delivered; a turn may end with several, the calls of one round, taken one by one in order.
    /// It first drives the generation as a poll does, without delivering, so a turn that ended
    /// since the last poll is seen. None while a turn runs or its text is still to be delivered,
    /// when it ended in prose only, or when every call of it was taken already.
    pub fn take_proposal(&mut self) -> Option<ToolCall> {
        let waker = self.waker.clone().unwrap_or_else(|| Waker::noop().clone());
        self.progress(&mut Context::from_waker(&waker));
        if self.turn.is_none() && self.delivered.is_empty() {
            self.staged.pop_front()
        } else {
            None
        }
    }

    /// Answers the call `call_id` of the latest turn with `output`, `failed` when it is a
    /// failure, as `llm`'s tool result, and wakes the latest poll. Once every call of the round
    /// is answered, the next turn runs on the conversation with the results appended; its text
    /// and any calls it ends with are delivered as for any turn.
    ///
    /// # Errors
    ///
    /// [`OutcomeRefused::GenerationStateConflict`] carrying the generation's state when it is not
    /// Active, and otherwise [`OutcomeRefused::NotAwaited`] when no unanswered call of the latest
    /// turn is `call_id`; nothing is integrated.
    pub fn integrate(
        &mut self,
        call_id: &CallId,
        output: Value,
        failed: bool,
    ) -> Result<(), OutcomeRefused> {
        self.output.require_active()?;
        let Some(position) = self.awaited.iter().position(|awaited| awaited == call_id) else {
            return Err(CallNotAwaited {
                call_id: call_id.clone(),
            }
            .into());
        };
        let call_id = self.awaited.remove(position);
        self.conversation.items.push(Item::ToolResult {
            call_id,
            output,
            failed,
        });
        self.answered = true;
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
        Ok(())
    }

    /// Runs `inference.stream.CompleteGeneration` now, whatever is unanswered: the running turn
    /// is cancelled, unanswered calls and waiting input are dropped, and what an adapter that
    /// does not stream held is delivered, then [`GenerationEvent::Completed`].
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the held state when the generation is not Active.
    pub fn complete(&mut self) -> Result<(), GenerationStateConflict> {
        self.output.require_active()?;
        self.finish();
        Ok(())
    }

    /// Runs `inference.stream.SupersedeGeneration`; the running turn is cancelled and nothing
    /// more is delivered.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the held state when the generation is not Active.
    pub fn supersede(&mut self) -> Result<GenerationSuperseded, GenerationStateConflict> {
        let (event, _) = self.output.supersede()?;
        self.stop();
        self.delivered.clear();
        self.held.clear();
        Ok(event)
    }

    /// Runs `inference.stream.CancelGeneration`; the running turn is cancelled and nothing more
    /// is delivered.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the held state when the generation is not Active.
    pub fn cancel(&mut self) -> Result<GenerationCancelled, GenerationStateConflict> {
        let (event, _) = self.output.cancel()?;
        self.stop();
        self.delivered.clear();
        self.held.clear();
        Ok(event)
    }

    /// Drives the generation without delivering: polls the running turn and starts the next one
    /// while one can start, until a turn is pending or none can start.
    fn progress(&mut self, cx: &mut Context<'_>) {
        while self.output.generation().state() == GenerationState::Active {
            if let Some(turn) = self.turn.as_mut() {
                let polled = turn.future.as_mut().poll(cx);
                self.take_accepted();
                match polled {
                    Poll::Ready(Ok(outcome)) => self.turn_ended(outcome),
                    Poll::Ready(Err(error)) => self.turn_failed(error),
                    Poll::Pending => return,
                }
            } else if self.can_start() {
                self.start_turn();
            } else {
                return;
            }
        }
    }

    /// Whether a turn can start: no call is unanswered, and there is something to ask: the
    /// request's own items before the first turn, waiting input, or results.
    fn can_start(&self) -> bool {
        self.awaited.is_empty()
            && ((self.first && !self.conversation.items.is_empty())
                || self.answered
                || !self.inputs.is_empty())
    }

    /// Starts the next turn on the conversation: the request as it was given when it carries
    /// items and no turn ran yet, the waiting inputs appended as user text otherwise.
    fn start_turn(&mut self) {
        let as_given = self.first && !self.conversation.items.is_empty();
        self.first = false;
        if !as_given {
            self.conversation
                .items
                .extend(self.inputs.drain(..).map(Item::user));
        }
        self.answered = false;
        self.shown = false;
        let request = self.conversation.clone();
        let model = Arc::clone(&self.model);
        let cancel = self.cancel.clone();
        let accepted = Accepted::default();
        let mut sink = Queue(Arc::clone(&accepted));
        let future = Box::pin(async move { model.turn(&request, &mut sink, &cancel).await });
        self.turn = Some(Turn { future, accepted });
    }

    /// Takes the text the running turn streamed since the last poll.
    fn take_accepted(&mut self) {
        let Some(turn) = self.turn.as_ref() else {
            return;
        };
        let accepted: Vec<StreamEvent> = turn
            .accepted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        for event in accepted {
            if let StreamEvent::TextDelta { text } = event
                && !text.is_empty()
            {
                self.shown = true;
                if self.streams {
                    self.segment(text);
                } else {
                    self.text.push_str(&text);
                }
            }
        }
    }

    /// Emits `text` as one segment and commits it, queueing both events for delivery, or, for
    /// an adapter that does not stream, holding them.
    fn segment(&mut self, text: String) {
        let Ok(delta) = self.output.emit(text) else {
            return;
        };
        let queue = if self.streams {
            &mut self.delivered
        } else {
            &mut self.held
        };
        queue.push_back(GenerationEvent::TextDelta(delta));
        if let Ok(committed) = self.output.commit_tentative() {
            queue.extend(committed.into_iter().map(GenerationEvent::SegmentCommitted));
        }
    }

    /// Takes the outcome of the turn that ended: its text, when no delta showed it, becomes one
    /// segment, its items join the conversation and its calls are staged. A turn ending with
    /// calls is the safe boundary at which an adapter that does not stream delivers what it held,
    /// so the caller can take the calls while the generation is Active.
    fn turn_ended(&mut self, outcome: TurnOutcome) {
        self.turn = None;
        let text = if self.shown {
            std::mem::take(&mut self.text)
        } else {
            outcome
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::AssistantText { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect()
        };
        self.shown = false;
        if !text.is_empty() {
            self.segment(text);
        }
        self.conversation
            .items
            .extend(outcome.items.iter().cloned());
        self.staged = outcome.tool_calls().cloned().collect();
        self.awaited = self
            .staged
            .iter()
            .map(|call| call.call_id.clone())
            .collect();
        if !self.staged.is_empty() {
            self.delivered.extend(self.held.drain(..));
        }
        self.outcomes.push(outcome);
    }

    /// Cancels the generation of a turn that failed, keeping `error`: what was delivered stays
    /// queued, and [`GenerationEvent::Cancelled`] follows it with the segments discarded.
    fn turn_failed(&mut self, error: llm_core::Error) {
        if let Ok((cancelled, discarded)) = self.output.cancel() {
            self.stop();
            self.held.clear();
            self.delivered
                .push_back(GenerationEvent::Cancelled(cancelled, discarded));
        }
        self.failure = Some(error);
    }

    /// Completes the generation, delivering what an adapter that does not stream held.
    fn finish(&mut self) {
        if let Ok(completed) = self.output.complete() {
            self.stop();
            self.delivered.extend(self.held.drain(..));
            self.delivered
                .push_back(GenerationEvent::Completed(completed));
        }
    }

    /// Cancels the running turn and drops what no later turn or delivery will take.
    fn stop(&mut self) {
        self.cancel.cancel();
        self.turn = None;
        self.text.clear();
        self.inputs.clear();
        self.staged.clear();
        self.awaited.clear();
    }
}

impl Drop for GenerationHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Wakes the thread that blocks on a generation's next event.
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

impl Iterator for GenerationHandle {
    type Item = GenerationEvent;

    /// Blocks the calling thread until the next event of [`GenerationHandle::poll_event`]; none
    /// while the generation awaits results, which only the caller can give. In an asynchronous
    /// task, await [`GenerationHandle::next_event`] instead.
    fn next(&mut self) -> Option<GenerationEvent> {
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(event) = self.poll_event(&mut cx) {
                return event;
            }
            self.turn.as_ref()?;
            thread::park();
        }
    }
}
