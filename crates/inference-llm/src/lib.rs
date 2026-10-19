//! The LLM contract of the inference framework.
//!
//! `inference-llm` is the generation contract over `inference-core`'s [`Generation`] and
//! `inference-text`'s [`OutputSegment`]. A [`Generator`] is an `inference.model.Adapter`: it
//! carries the [`Adapter`] record it was declared with, and [`Generator::start`] begins one
//! generation for a [`GenerationRequest`] under the run and producing node the caller names. The
//! returned [`GenerationHandle`] yields the generation's [`GenerationEvent`]s, the outbound
//! commands of `ess/domains/stream.yaml`: `StartGeneration`, then `EmitTextDelta` and
//! `CommitSegment` for each segment, then `CompleteGeneration`. `CancelGeneration` and
//! `SupersedeGeneration` are control, called on the handle.
//!
//! [`Generator::start`] refuses a start past the maximum concurrency the adapter record declares
//! with [`ConcurrencyLimitReached`]; a generation runs until its handle is dropped.
//!
//! Output streaming, incremental input and input during output are distinct capabilities
//! (`inference.model.Capability`), and a persistent connection proves none of them. What the
//! contract can observe, it decides here:
//!
//! * A generator streams output when a [`TextDelta`] reaches the caller while the generation is
//!   still Active. One that does not stream delivers its segments only once the generation is
//!   Completed; both emit every segment into the Active generation, as `EmitTextDelta` requires.
//! * Input offered while a generation is Active is admitted by [`GenerationOutput::admit_input`]
//!   only when the adapter claims [`Capability::ConcurrentInputOutput`], and refused otherwise
//!   with [`CapabilityNotClaimed`] naming it. A handle takes input only as the [`AdmittedInput`]
//!   that admission returns, and the text inside is reachable only through
//!   [`GenerationOutput::release`] of the generation that admitted it, while that generation is
//!   still Active; input admitted elsewhere is refused with [`AdmittedByAnotherGeneration`]. So no
//!   implementation accepts input its adapter does not claim, and an admission is taken once.
//!
//! [`GenerationOutput`] is the bookkeeping every implementation shares: the generation and the
//! segments it owns, moved only through the transitions of `inference-core` and
//! `inference-text`. The crate carries no concrete model, backend or provider; the scripted mock
//! generator is `inference-testkit`'s, behind its `llm` feature. Tool proposals, context
//! semantics and token accounting are not part of the contract yet.

use std::error::Error;
use std::fmt;

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

/// The revision of this contract, for the `contract_revision` an adapter implementing it
/// declares.
pub const CONTRACT_REVISION: &str = env!("CARGO_PKG_VERSION");

/// A request for one generation: the committed `prompt` text, and the run, producing node and
/// epoch the generation is started under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationRequest {
    pub run_id: RunId,
    pub node_id: NodeId,
    pub epoch: i64,
    pub prompt: String,
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

/// An implementation of the LLM contract: an `inference.model.Adapter` that generates text.
pub trait Generator {
    /// The handle of one running generation.
    type Handle: GenerationHandle;

    /// The adapter record this generator was declared with, the capabilities it claims among it.
    fn adapter(&self) -> &Adapter;

    /// Starts one generation for `request`; the handle yields its events, the first
    /// [`GenerationEvent::Started`]. A generation runs until its handle is dropped.
    ///
    /// # Errors
    ///
    /// [`ConcurrencyLimitReached`] carrying the adapter's `max_concurrency` when that many
    /// generations of this generator already run; nothing starts.
    fn start(
        &mut self,
        request: GenerationRequest,
    ) -> Result<Self::Handle, ConcurrencyLimitReached>;
}

/// One running generation. Iterating it yields the generation's events as the implementation
/// delivers them, and [`GenerationHandle::output`] shows where the generation stands at each.
/// Iteration ends after [`GenerationEvent::Completed`], or once the generation is superseded or
/// cancelled.
///
/// Input reaches an implementation only through [`GenerationHandle::accept_input`], as an
/// [`AdmittedInput`]; a caller offers text through [`OfferInput::offer_input`], which every
/// handle has and none defines. An implementation:
///
/// ```
/// # use inference_core::generation::{GenerationCancelled, GenerationStateConflict, GenerationSuperseded};
/// # use inference_llm::{AdmittedInput, GenerationEvent, GenerationHandle, GenerationOutput, InputRefused};
/// struct Handle {
///     output: GenerationOutput,
///     taken: Vec<String>,
/// }
///
/// impl Iterator for Handle {
///     type Item = GenerationEvent;
///     fn next(&mut self) -> Option<GenerationEvent> {
///         None
///     }
/// }
///
/// impl GenerationHandle for Handle {
///     fn output(&self) -> &GenerationOutput {
///         &self.output
///     }
///     fn accept_input(&mut self, input: AdmittedInput) -> Result<(), InputRefused> {
///         let text = self.output.release(input)?;
///         self.taken.push(text);
///         Ok(())
///     }
///     fn supersede(&mut self) -> Result<GenerationSuperseded, GenerationStateConflict> {
///         Ok(self.output.supersede()?.0)
///     }
///     fn cancel(&mut self) -> Result<GenerationCancelled, GenerationStateConflict> {
///         Ok(self.output.cancel()?.0)
///     }
/// }
/// ```
///
/// The same implementation defining an `offer_input` that takes raw text does not compile:
///
/// ```compile_fail
/// # use inference_core::generation::{GenerationCancelled, GenerationStateConflict, GenerationSuperseded};
/// # use inference_llm::{AdmittedInput, GenerationEvent, GenerationHandle, GenerationOutput, InputRefused};
/// # struct Handle {
/// #     output: GenerationOutput,
/// #     taken: Vec<String>,
/// # }
/// # impl Iterator for Handle {
/// #     type Item = GenerationEvent;
/// #     fn next(&mut self) -> Option<GenerationEvent> {
/// #         None
/// #     }
/// # }
/// impl GenerationHandle for Handle {
///     fn output(&self) -> &GenerationOutput {
///         &self.output
///     }
///     fn accept_input(&mut self, input: AdmittedInput) -> Result<(), InputRefused> {
///         let text = self.output.release(input)?;
///         self.taken.push(text);
///         Ok(())
///     }
///     fn supersede(&mut self) -> Result<GenerationSuperseded, GenerationStateConflict> {
///         Ok(self.output.supersede()?.0)
///     }
///     fn cancel(&mut self) -> Result<GenerationCancelled, GenerationStateConflict> {
///         Ok(self.output.cancel()?.0)
///     }
///     fn offer_input(&mut self, text: String) -> Result<(), InputRefused> {
///         self.taken.push(text);
///         Ok(())
///     }
/// }
/// ```
pub trait GenerationHandle: Iterator<Item = GenerationEvent> {
    /// The generation and its segments, as they stand now.
    fn output(&self) -> &GenerationOutput;

    /// Takes input admitted into the running generation, through [`GenerationOutput::release`],
    /// and reflects it in later output; [`OfferInput::offer_input`] is how a caller reaches it.
    ///
    /// # Errors
    ///
    /// The [`InputRefused`] of [`GenerationOutput::release`]: input admitted by another
    /// generation, or a generation no longer Active; nothing is accepted.
    fn accept_input(&mut self, input: AdmittedInput) -> Result<(), InputRefused>;

    /// Runs `inference.stream.SupersedeGeneration`; nothing more is delivered.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the held state when the generation is not Active.
    fn supersede(&mut self) -> Result<GenerationSuperseded, GenerationStateConflict>;

    /// Runs `inference.stream.CancelGeneration`; nothing more is delivered.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the held state when the generation is not Active.
    fn cancel(&mut self) -> Result<GenerationCancelled, GenerationStateConflict>;
}

mod sealed {
    /// Implemented for every [`GenerationHandle`](super::GenerationHandle) and nothing else, so
    /// that [`OfferInput`](super::OfferInput) has one implementation.
    pub trait Sealed {}

    impl<T: super::GenerationHandle + ?Sized> Sealed for T {}
}

/// How a caller offers text to a running generation: admitted by
/// [`GenerationOutput::admit_input`], then accepted by [`GenerationHandle::accept_input`]. It is
/// implemented once, for every [`GenerationHandle`], and sealed, so no implementation replaces
/// the path:
///
/// ```compile_fail
/// struct Elsewhere;
///
/// impl inference_llm::OfferInput for Elsewhere {
///     fn offer_input(&mut self, _text: String) -> Result<(), inference_llm::InputRefused> {
///         Ok(())
///     }
/// }
/// ```
pub trait OfferInput: sealed::Sealed {
    /// Offers `text` to the running generation.
    ///
    /// # Errors
    ///
    /// The [`InputRefused`] of admission or acceptance; nothing is accepted.
    fn offer_input(&mut self, text: String) -> Result<(), InputRefused>;
}

impl<T: GenerationHandle + ?Sized> OfferInput for T {
    fn offer_input(&mut self, text: String) -> Result<(), InputRefused> {
        let input = self.output().admit_input(text)?;
        self.accept_input(input)
    }
}
