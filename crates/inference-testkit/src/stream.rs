//! The generations and output segments of `ess/domains/stream.yaml`, held in memory.
//!
//! [`StreamDomain`] holds one store per entity, [`GenerationStore`] and [`SegmentStore`], and
//! records every event it emits in order. Its commands run the transitions of `inference-core`'s
//! [`Generation`] and `inference-text`'s [`OutputSegment`] under identities the domain issues from
//! a counter rendered as a Uuid, `00000000-0000-4000-8000-000000000001` first, so that two runs of
//! the same commands report the same identities. What only a store can decide is decided here: a
//! command naming an identity no record carries is refused before anything moves, and
//! `SupersedeGeneration` and `CancelGeneration` discard every unplayed segment of the generation in
//! the same outcome, leaving a Played one as it is. The cascade emits no event of its own: the
//! outcome emits its generation event only, as `ess/domains/stream.yaml` declares it. The views
//! `inference.stream.Generations` and `inference.stream.Segments` are
//! [`StreamDomain::generations_view`] and [`StreamDomain::segments_view`]; both are
//! `read_your_writes`, and every row is read from the stores at the call.
//!
//! `StartGeneration` refuses a run no store holds, a run that is not Active and a node no
//! pipeline declares. Those are reads of the `inference.graph` stores, which do not exist yet;
//! [`StreamDomain::start_generation`] runs the `started` outcome only, for a caller that has
//! decided the three refusals against them.
//!
//! [`StreamDomain`]: crate::stream::StreamDomain
//! [`GenerationStore`]: crate::stream::GenerationStore
//! [`SegmentStore`]: crate::stream::SegmentStore
//! [`Generation`]: inference_core::generation::Generation
//! [`OutputSegment`]: inference_text::OutputSegment
//! [`StreamDomain::generations_view`]: crate::stream::StreamDomain::generations_view
//! [`StreamDomain::segments_view`]: crate::stream::StreamDomain::segments_view
//! [`StreamDomain::start_generation`]: crate::stream::StreamDomain::start_generation

use std::error::Error;
use std::fmt;

use uuid::Uuid;

use inference_core::generation::{
    Generation, GenerationCancelled, GenerationCompleted, GenerationNotFound, GenerationStarted,
    GenerationState, GenerationStateConflict, GenerationSuperseded, StartGeneration,
};
use inference_core::id::{GenerationId, NodeId, RunId};
use inference_text::{
    AudioPlayed, EmitTextDelta, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued,
    SegmentState, SegmentStateConflict, SynthesisCompleted, TextDelta, TextSegmentCommitted,
};

/// The store of `inference.stream.Generation`, in the order the generations were started.
#[derive(Debug, Clone, Default)]
pub struct GenerationStore {
    generations: Vec<Generation>,
}

impl GenerationStore {
    /// The generation carrying `generation_id`, if one was started.
    #[must_use]
    pub fn get(&self, generation_id: GenerationId) -> Option<&Generation> {
        self.generations
            .iter()
            .find(|generation| generation.generation_id() == generation_id)
    }

    /// Every generation, in the order they were started.
    pub fn iter(&self) -> impl Iterator<Item = &Generation> {
        self.generations.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.generations.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.generations.is_empty()
    }
}

/// The store of `inference.stream.OutputSegment`, in the order the segments were emitted.
#[derive(Debug, Clone, Default)]
pub struct SegmentStore {
    segments: Vec<OutputSegment>,
}

impl SegmentStore {
    /// The segment carrying `segment_id`, if one was emitted.
    #[must_use]
    pub fn get(&self, segment_id: SegmentId) -> Option<&OutputSegment> {
        self.segments
            .iter()
            .find(|segment| segment.segment_id() == segment_id)
    }

    /// Every segment, in the order they were emitted.
    pub fn iter(&self) -> impl Iterator<Item = &OutputSegment> {
        self.segments.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

/// An event of the generations and segments of `inference.stream`, as the domain emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// `inference.stream.GenerationStarted`.
    GenerationStarted(GenerationStarted),
    /// `inference.stream.GenerationCompleted`.
    GenerationCompleted(GenerationCompleted),
    /// `inference.stream.GenerationSuperseded`.
    GenerationSuperseded(GenerationSuperseded),
    /// `inference.stream.GenerationCancelled`.
    GenerationCancelled(GenerationCancelled),
    /// `inference.stream.TextDelta`.
    TextDelta(TextDelta),
    /// `inference.stream.TextSegmentCommitted`.
    TextSegmentCommitted(TextSegmentCommitted),
    /// `inference.stream.SegmentQueued`.
    SegmentQueued(SegmentQueued),
    /// `inference.stream.SynthesisCompleted`.
    SynthesisCompleted(SynthesisCompleted),
    /// `inference.stream.AudioPlayed`.
    AudioPlayed(AudioPlayed),
    /// `inference.stream.SegmentDiscarded`.
    SegmentDiscarded(SegmentDiscarded),
}

/// Why `CompleteGeneration`, `SupersedeGeneration` or `CancelGeneration` moved nothing. Both are
/// the command's `wrong-state` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationMoveError {
    /// No stored generation carries the identity sent; there is no state to report.
    Unknown(GenerationNotFound),
    /// The generation is not Active.
    Conflict(GenerationStateConflict),
}

impl fmt::Display for GenerationMoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(error) => fmt::Display::fmt(error, f),
            Self::Conflict(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for GenerationMoveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unknown(error) => Some(error),
            Self::Conflict(error) => Some(error),
        }
    }
}

/// Why `inference.stream.EmitTextDelta` created no segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitTextDeltaError {
    /// The `unknown-generation` outcome: no stored generation carries the identity sent.
    UnknownGeneration(GenerationNotFound),
    /// The `stale-generation` outcome: the generation is not Active.
    StaleGeneration(GenerationStateConflict),
}

impl fmt::Display for EmitTextDeltaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownGeneration(error) => fmt::Display::fmt(error, f),
            Self::StaleGeneration(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for EmitTextDeltaError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownGeneration(error) => Some(error),
            Self::StaleGeneration(error) => Some(error),
        }
    }
}

/// Why a segment command moved nothing. Both are the command's `wrong-state` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentMoveError {
    /// No stored segment carries `segment_id`; there is no state to report.
    /// `ess/domains/stream.yaml` declares no not-found error for a segment.
    Unknown { segment_id: SegmentId },
    /// The segment is not in a state the command acts from.
    Conflict(SegmentStateConflict),
}

impl fmt::Display for SegmentMoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { segment_id } => {
                write!(f, "no segment carries the identity {segment_id}")
            }
            Self::Conflict(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for SegmentMoveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unknown { .. } => None,
            Self::Conflict(error) => Some(error),
        }
    }
}

/// A row of the view `inference.stream.Generations`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationsRow {
    pub generation_id: GenerationId,
    pub run_id: RunId,
    pub node_id: NodeId,
    pub epoch: i64,
    pub state: GenerationState,
}

/// A row of the view `inference.stream.Segments`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentsRow {
    pub segment_id: SegmentId,
    pub generation_id: GenerationId,
    pub run_id: RunId,
    pub sequence: i64,
    pub text: String,
    pub state: SegmentState,
}

/// The generations and segments of `inference.stream` in memory: their two entity stores and the
/// events emitted.
#[derive(Debug, Clone, Default)]
pub struct StreamDomain {
    generations: GenerationStore,
    segments: SegmentStore,
    events: Vec<StreamEvent>,
    issued: u128,
}

impl StreamDomain {
    /// An empty domain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs the `started` outcome of `inference.stream.StartGeneration`: stores the generation
    /// [`Generation::start_as`] returns and records its [`GenerationStarted`]. The run and node
    /// refusals are the caller's, decided before it calls this.
    pub fn start_generation(&mut self, input: StartGeneration) -> GenerationStarted {
        let generation_id = GenerationId::from(self.issue());
        let (generation, event) = Generation::start_as(generation_id, input);
        self.generations.generations.push(generation);
        self.events.push(StreamEvent::GenerationStarted(event));
        event
    }

    /// Runs `inference.stream.CompleteGeneration`.
    ///
    /// # Errors
    ///
    /// [`GenerationMoveError::Unknown`] when no stored generation carries `generation_id`,
    /// [`GenerationMoveError::Conflict`] when it is not Active. Nothing moves and no event is
    /// recorded.
    pub fn complete_generation(
        &mut self,
        generation_id: GenerationId,
    ) -> Result<GenerationCompleted, GenerationMoveError> {
        self.move_generation(
            generation_id,
            Generation::complete,
            StreamEvent::GenerationCompleted,
        )
    }

    /// Runs `inference.stream.SupersedeGeneration`: the generation moves to Superseded and every
    /// segment of it that is not Played or already Discarded moves to Discarded, in one outcome
    /// that records the [`GenerationSuperseded`] only.
    ///
    /// # Errors
    ///
    /// As [`StreamDomain::complete_generation`]; no generation or segment moves.
    pub fn supersede_generation(
        &mut self,
        generation_id: GenerationId,
    ) -> Result<GenerationSuperseded, GenerationMoveError> {
        let event = self.move_generation(
            generation_id,
            Generation::supersede,
            StreamEvent::GenerationSuperseded,
        )?;
        self.discard_unplayed(generation_id);
        Ok(event)
    }

    /// Runs `inference.stream.CancelGeneration`: the generation moves to Cancelled and its
    /// unplayed segments to Discarded, as [`StreamDomain::supersede_generation`] does, recording
    /// the [`GenerationCancelled`] only.
    ///
    /// # Errors
    ///
    /// As [`StreamDomain::complete_generation`]; no generation or segment moves.
    pub fn cancel_generation(
        &mut self,
        generation_id: GenerationId,
    ) -> Result<GenerationCancelled, GenerationMoveError> {
        let event = self.move_generation(
            generation_id,
            Generation::cancel,
            StreamEvent::GenerationCancelled,
        )?;
        self.discard_unplayed(generation_id);
        Ok(event)
    }

    /// Runs `inference.stream.EmitTextDelta`: stores the Tentative segment
    /// [`OutputSegment::emit_as`] returns and records its [`TextDelta`].
    ///
    /// # Errors
    ///
    /// [`EmitTextDeltaError::UnknownGeneration`] when no stored generation carries
    /// `generation_id`, otherwise [`EmitTextDeltaError::StaleGeneration`] when it is not Active.
    /// Nothing is stored, no event is recorded and no identity is issued.
    pub fn emit_text_delta(
        &mut self,
        input: EmitTextDelta,
    ) -> Result<TextDelta, EmitTextDeltaError> {
        let generation = self.generations.get(input.generation_id).ok_or(
            EmitTextDeltaError::UnknownGeneration(GenerationNotFound {
                generation_id: input.generation_id,
            }),
        )?;
        let segment_id = SegmentId::from(Self::identity(self.issued + 1));
        let (segment, event) =
            OutputSegment::emit_as(segment_id, generation, input.sequence, input.text)
                .map_err(EmitTextDeltaError::StaleGeneration)?;
        self.issue();
        self.segments.segments.push(segment);
        self.events.push(StreamEvent::TextDelta(event.clone()));
        Ok(event)
    }

    /// Runs `inference.stream.CommitSegment`.
    ///
    /// # Errors
    ///
    /// [`SegmentMoveError::Unknown`] when no stored segment carries `segment_id`,
    /// [`SegmentMoveError::Conflict`] when it is not Tentative.
    pub fn commit_segment(
        &mut self,
        segment_id: SegmentId,
    ) -> Result<TextSegmentCommitted, SegmentMoveError> {
        self.move_segment(
            segment_id,
            OutputSegment::commit,
            StreamEvent::TextSegmentCommitted,
        )
    }

    /// Runs `inference.stream.QueueForSynthesis`.
    ///
    /// # Errors
    ///
    /// [`SegmentMoveError::Unknown`] when no stored segment carries `segment_id`,
    /// [`SegmentMoveError::Conflict`] when it is not Committed.
    pub fn queue_for_synthesis(
        &mut self,
        segment_id: SegmentId,
    ) -> Result<SegmentQueued, SegmentMoveError> {
        self.move_segment(segment_id, OutputSegment::queue, StreamEvent::SegmentQueued)
    }

    /// Runs `inference.stream.CompleteSynthesis`.
    ///
    /// # Errors
    ///
    /// [`SegmentMoveError::Unknown`] when no stored segment carries `segment_id`,
    /// [`SegmentMoveError::Conflict`] when it is not Queued.
    pub fn complete_synthesis(
        &mut self,
        segment_id: SegmentId,
    ) -> Result<SynthesisCompleted, SegmentMoveError> {
        self.move_segment(
            segment_id,
            OutputSegment::synthesize,
            StreamEvent::SynthesisCompleted,
        )
    }

    /// Runs `inference.stream.AcknowledgePlayback`.
    ///
    /// # Errors
    ///
    /// [`SegmentMoveError::Unknown`] when no stored segment carries `segment_id`,
    /// [`SegmentMoveError::Conflict`] when it is not Synthesized.
    pub fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
    ) -> Result<AudioPlayed, SegmentMoveError> {
        self.move_segment(segment_id, OutputSegment::play, StreamEvent::AudioPlayed)
    }

    /// Runs `inference.stream.DiscardSegment`.
    ///
    /// # Errors
    ///
    /// [`SegmentMoveError::Unknown`] when no stored segment carries `segment_id`,
    /// [`SegmentMoveError::Conflict`] when it is Played or already Discarded.
    pub fn discard_segment(
        &mut self,
        segment_id: SegmentId,
    ) -> Result<SegmentDiscarded, SegmentMoveError> {
        self.move_segment(
            segment_id,
            OutputSegment::discard,
            StreamEvent::SegmentDiscarded,
        )
    }

    /// Moves the generation carrying `generation_id` by `transition`, stores it and records its
    /// event.
    fn move_generation<E: Clone>(
        &mut self,
        generation_id: GenerationId,
        transition: fn(&Generation) -> Result<(Generation, E), GenerationStateConflict>,
        record: fn(E) -> StreamEvent,
    ) -> Result<E, GenerationMoveError> {
        let slot = self
            .generations
            .generations
            .iter_mut()
            .find(|generation| generation.generation_id() == generation_id)
            .ok_or(GenerationMoveError::Unknown(GenerationNotFound {
                generation_id,
            }))?;
        let (moved, event) = transition(slot).map_err(GenerationMoveError::Conflict)?;
        *slot = moved;
        self.events.push(record(event.clone()));
        Ok(event)
    }

    /// Moves every segment of `generation_id` through `discard`; a Played or already Discarded
    /// one is outside the transition's source states and is skipped. No event is recorded: the
    /// outcome that discards them emits its generation event only.
    fn discard_unplayed(&mut self, generation_id: GenerationId) {
        for segment in &mut self.segments.segments {
            if segment.generation_id() == generation_id
                && let Ok((discarded, _)) = segment.discard()
            {
                *segment = discarded;
            }
        }
    }

    /// Moves the segment carrying `segment_id` by `transition`, stores it and records its event.
    fn move_segment<E: Clone>(
        &mut self,
        segment_id: SegmentId,
        transition: fn(&OutputSegment) -> Result<(OutputSegment, E), SegmentStateConflict>,
        record: fn(E) -> StreamEvent,
    ) -> Result<E, SegmentMoveError> {
        let slot = self
            .segments
            .segments
            .iter_mut()
            .find(|segment| segment.segment_id() == segment_id)
            .ok_or(SegmentMoveError::Unknown { segment_id })?;
        let (moved, event) = transition(slot).map_err(SegmentMoveError::Conflict)?;
        *slot = moved;
        self.events.push(record(event.clone()));
        Ok(event)
    }

    /// The next identity of the counter; a refused command takes none.
    fn issue(&mut self) -> Uuid {
        self.issued += 1;
        Self::identity(self.issued)
    }

    /// The `count`th identity: a version 4, RFC 4122 variant Uuid whose low bits are `count`,
    /// `00000000-0000-4000-8000-000000000001` first.
    const fn identity(count: u128) -> Uuid {
        Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | count)
    }

    /// The started generations.
    #[must_use]
    pub const fn generations(&self) -> &GenerationStore {
        &self.generations
    }

    /// The emitted segments.
    #[must_use]
    pub const fn segments(&self) -> &SegmentStore {
        &self.segments
    }

    /// Every event emitted, in the order it was emitted.
    #[must_use]
    pub fn events(&self) -> &[StreamEvent] {
        &self.events
    }

    /// The view `inference.stream.Generations`: one row per generation, in the order they were
    /// started.
    #[must_use]
    pub fn generations_view(&self) -> Vec<GenerationsRow> {
        self.generations
            .iter()
            .map(|generation| GenerationsRow {
                generation_id: generation.generation_id(),
                run_id: generation.run_id(),
                node_id: generation.node_id(),
                epoch: generation.epoch(),
                state: generation.state(),
            })
            .collect()
    }

    /// The view `inference.stream.Segments`: one row per segment, in the order they were emitted.
    #[must_use]
    pub fn segments_view(&self) -> Vec<SegmentsRow> {
        self.segments
            .iter()
            .map(|segment| SegmentsRow {
                segment_id: segment.segment_id(),
                generation_id: segment.generation_id(),
                run_id: segment.run_id(),
                sequence: segment.sequence(),
                text: segment.text().to_owned(),
                state: segment.state(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use inference_core::generation::{
        GenerationNotFound, GenerationState, GenerationStateConflict, StartGeneration,
    };
    use inference_core::id::{GenerationId, NodeId, RunId};
    use inference_text::{EmitTextDelta, SegmentId, SegmentState, SegmentStateConflict};
    use uuid::Uuid;

    use super::{
        EmitTextDeltaError, GenerationMoveError, GenerationsRow, SegmentMoveError, SegmentsRow,
        StreamDomain, StreamEvent,
    };

    fn run_id() -> RunId {
        RunId::from(Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d))
    }

    fn node_id() -> NodeId {
        NodeId::from(Uuid::from_u128(0x1a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d))
    }

    fn start(domain: &mut StreamDomain) -> GenerationId {
        domain
            .start_generation(StartGeneration {
                run_id: run_id(),
                node_id: node_id(),
                epoch: 1,
            })
            .generation_id
    }

    fn emit(domain: &mut StreamDomain, generation_id: GenerationId, sequence: i64) -> SegmentId {
        domain
            .emit_text_delta(EmitTextDelta {
                generation_id,
                sequence,
                text: format!("text {sequence}"),
            })
            .expect("an Active generation admits a delta")
            .segment_id
    }

    fn state(domain: &StreamDomain, segment_id: SegmentId) -> SegmentState {
        domain.segments().get(segment_id).expect("stored").state()
    }

    /// One generation holding a segment in each of the six states, keyed by that state.
    fn one_of_each(domain: &mut StreamDomain) -> (GenerationId, [(SegmentState, SegmentId); 6]) {
        let generation = start(domain);
        let tentative = emit(domain, generation, 1);
        let committed = emit(domain, generation, 2);
        let queued = emit(domain, generation, 3);
        let synthesized = emit(domain, generation, 4);
        let played = emit(domain, generation, 5);
        let discarded = emit(domain, generation, 6);
        for segment in [committed, queued, synthesized, played] {
            domain.commit_segment(segment).expect("commits");
        }
        for segment in [queued, synthesized, played] {
            domain.queue_for_synthesis(segment).expect("queues");
        }
        for segment in [synthesized, played] {
            domain.complete_synthesis(segment).expect("synthesizes");
        }
        domain.acknowledge_playback(played).expect("plays");
        domain.discard_segment(discarded).expect("discards");
        (
            generation,
            [
                (SegmentState::Tentative, tentative),
                (SegmentState::Committed, committed),
                (SegmentState::Queued, queued),
                (SegmentState::Synthesized, synthesized),
                (SegmentState::Played, played),
                (SegmentState::Discarded, discarded),
            ],
        )
    }

    #[test]
    fn identities_come_from_one_counter_and_a_refusal_takes_none() {
        let run = || {
            let mut domain = StreamDomain::new();
            let generation = start(&mut domain);
            let unknown = GenerationId::from(Uuid::from_u128(0xff));
            let refused = domain.emit_text_delta(EmitTextDelta {
                generation_id: unknown,
                sequence: 1,
                text: "lost".to_owned(),
            });
            let segment = emit(&mut domain, generation, 1);
            (generation, refused.is_err(), segment)
        };
        let (generation, refused, segment) = run();
        assert_eq!(
            generation.to_string(),
            "00000000-0000-4000-8000-000000000001"
        );
        assert!(refused);
        assert_eq!(segment.to_string(), "00000000-0000-4000-8000-000000000002");
        assert_eq!(run(), (generation, refused, segment));
    }

    #[test]
    fn supersede_discards_every_unplayed_segment_of_the_generation_in_the_same_outcome() {
        let mut domain = StreamDomain::new();
        let (generation, segments) = one_of_each(&mut domain);
        let other = start(&mut domain);
        let bystander = emit(&mut domain, other, 1);
        let events_before = domain.events().len();

        let event = domain
            .supersede_generation(generation)
            .expect("Active is superseded");

        assert_eq!(event.generation_id, generation);
        assert_eq!(
            domain
                .generations()
                .get(generation)
                .expect("stored")
                .state(),
            GenerationState::Superseded
        );
        for (before, segment) in segments {
            let expected = if before == SegmentState::Played {
                SegmentState::Played
            } else {
                SegmentState::Discarded
            };
            assert_eq!(state(&domain, segment), expected, "a {before:?} segment");
        }
        assert_eq!(
            state(&domain, bystander),
            SegmentState::Tentative,
            "another generation's segment is not touched"
        );
        assert_eq!(
            &domain.events()[events_before..],
            [StreamEvent::GenerationSuperseded(event)],
            "the outcome emits its generation event only"
        );
    }

    #[test]
    fn cancel_discards_every_unplayed_segment_of_the_generation_in_the_same_outcome() {
        let mut domain = StreamDomain::new();
        let (generation, segments) = one_of_each(&mut domain);
        let other = start(&mut domain);
        let bystander = emit(&mut domain, other, 1);
        let events_before = domain.events().len();

        let event = domain
            .cancel_generation(generation)
            .expect("Active is cancelled");

        assert_eq!(
            domain
                .generations()
                .get(generation)
                .expect("stored")
                .state(),
            GenerationState::Cancelled
        );
        for (before, segment) in segments {
            let expected = if before == SegmentState::Played {
                SegmentState::Played
            } else {
                SegmentState::Discarded
            };
            assert_eq!(state(&domain, segment), expected, "a {before:?} segment");
        }
        assert_eq!(state(&domain, bystander), SegmentState::Tentative);
        assert_eq!(
            &domain.events()[events_before..],
            [StreamEvent::GenerationCancelled(event)]
        );
    }

    #[test]
    fn complete_leaves_the_segments_as_they_are() {
        let mut domain = StreamDomain::new();
        let (generation, segments) = one_of_each(&mut domain);

        domain
            .complete_generation(generation)
            .expect("Active completes");

        for (before, segment) in segments {
            assert_eq!(state(&domain, segment), before);
        }
    }

    #[test]
    fn a_refused_generation_move_moves_no_generation_and_no_segment() {
        let mut domain = StreamDomain::new();
        let generation = start(&mut domain);
        domain
            .complete_generation(generation)
            .expect("Active completes");
        let snapshot = domain.segments_view();
        let events = domain.events().len();
        let conflict = GenerationMoveError::Conflict(GenerationStateConflict {
            current: GenerationState::Completed,
        });

        assert_eq!(domain.supersede_generation(generation), Err(conflict));
        assert_eq!(domain.cancel_generation(generation), Err(conflict));
        assert_eq!(domain.complete_generation(generation), Err(conflict));
        assert_eq!(domain.segments_view(), snapshot);
        assert_eq!(domain.events().len(), events);

        let unknown = GenerationId::from(Uuid::from_u128(0xff));
        let not_found = GenerationMoveError::Unknown(GenerationNotFound {
            generation_id: unknown,
        });
        assert_eq!(domain.complete_generation(unknown), Err(not_found));
        assert_eq!(domain.supersede_generation(unknown), Err(not_found));
        assert_eq!(domain.cancel_generation(unknown), Err(not_found));
        assert_eq!(domain.events().len(), events);
    }

    #[test]
    fn a_delta_names_a_stored_active_generation_or_is_refused() {
        let mut domain = StreamDomain::new();
        let unknown = GenerationId::from(Uuid::from_u128(0xff));
        assert_eq!(
            domain.emit_text_delta(EmitTextDelta {
                generation_id: unknown,
                sequence: 1,
                text: "lost".to_owned(),
            }),
            Err(EmitTextDeltaError::UnknownGeneration(GenerationNotFound {
                generation_id: unknown
            }))
        );

        let generation = start(&mut domain);
        domain
            .supersede_generation(generation)
            .expect("Active is superseded");
        assert_eq!(
            domain.emit_text_delta(EmitTextDelta {
                generation_id: generation,
                sequence: 1,
                text: "stale".to_owned(),
            }),
            Err(EmitTextDeltaError::StaleGeneration(
                GenerationStateConflict {
                    current: GenerationState::Superseded
                }
            ))
        );
        assert!(domain.segments().is_empty());
    }

    #[test]
    fn a_segment_command_on_an_unknown_or_wrong_state_segment_moves_nothing() {
        let mut domain = StreamDomain::new();
        let unknown = SegmentId::from(Uuid::from_u128(0xff));
        let not_found = SegmentMoveError::Unknown {
            segment_id: unknown,
        };
        assert_eq!(domain.commit_segment(unknown), Err(not_found));
        assert_eq!(domain.queue_for_synthesis(unknown), Err(not_found));
        assert_eq!(domain.complete_synthesis(unknown), Err(not_found));
        assert_eq!(domain.acknowledge_playback(unknown), Err(not_found));
        assert_eq!(domain.discard_segment(unknown), Err(not_found));
        assert!(domain.events().is_empty());

        let generation = start(&mut domain);
        let segment = emit(&mut domain, generation, 1);
        let events = domain.events().len();
        let tentative = SegmentMoveError::Conflict(SegmentStateConflict {
            current: SegmentState::Tentative,
        });
        assert_eq!(domain.queue_for_synthesis(segment), Err(tentative));
        assert_eq!(domain.complete_synthesis(segment), Err(tentative));
        assert_eq!(domain.acknowledge_playback(segment), Err(tentative));
        assert_eq!(state(&domain, segment), SegmentState::Tentative);
        assert_eq!(domain.events().len(), events);
    }

    #[test]
    fn the_views_read_the_stores_with_their_state() {
        let mut domain = StreamDomain::new();
        let generation = start(&mut domain);
        let segment = emit(&mut domain, generation, 4);
        domain.commit_segment(segment).expect("commits");

        assert_eq!(
            domain.generations_view(),
            [GenerationsRow {
                generation_id: generation,
                run_id: run_id(),
                node_id: node_id(),
                epoch: 1,
                state: GenerationState::Active,
            }]
        );
        assert_eq!(
            domain.segments_view(),
            [SegmentsRow {
                segment_id: segment,
                generation_id: generation,
                run_id: run_id(),
                sequence: 4,
                text: "text 4".to_owned(),
                state: SegmentState::Committed,
            }]
        );
    }
}
