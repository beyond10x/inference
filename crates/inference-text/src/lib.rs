//! The text vocabulary of the inference framework.
//!
//! `inference-text` holds the inbound text of `ess/domains/stream.yaml`: a [`Span`] is a unit of
//! inbound text, a Hypothesis until committed and immutable after, within a [`Turn`], whose
//! completion is a control boundary distinct from the commit of its spans.
//!
//! It holds the outbound text of `ess/domains/stream.yaml` too: an [`OutputSegment`] is
//! a unit of text a [`Generation`] produced, Tentative until committed, then Queued for synthesis,
//! then Synthesized, then Played once the media sink acknowledges it. Synthesis consumes committed
//! segments only, so a Tentative segment refuses to be queued; a Queued segment refuses playback
//! until its synthesis completes; and a Played segment cannot be unsaid, so it refuses to be
//! discarded. Each transition is a method that consumes the segment and returns the moved one and
//! the event its command emits, or a [`SegmentMoveRefused`] handing the segment back unmoved with
//! the [`SegmentStateConflict`] carrying the state it holds; a refused move changes nothing. Since
//! a transition consumes what it moves, a segment leaves each state once: a Tentative segment
//! commits once, so a segment made by [`OutputSegment::emit`] yields at most one Committed
//! value. [`OutputSegment::emit_as`] takes the identity from its caller, so two values of one
//! identity can still be made through it. The crate depends on
//! `inference-core` only and carries no synthesiser, sink or other concrete implementation.
//! Serialized names are the ESS names.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use inference_core::generation::{Generation, GenerationState, GenerationStateConflict};
use inference_core::id::{GenerationId, RunId};

mod inbound;

pub use inbound::{
    CommittedSpanImmutable, EmitHypothesis, OpenTurn, ReviseSpan, Span, SpanId, SpanNotFound,
    SpanState, SpanStateConflict, TranscriptCommitted, TranscriptHypothesis, TranscriptRevision,
    Turn, TurnCompleted, TurnId, TurnNotFound, TurnOpened, TurnState, TurnStateConflict,
};

inference_core::uuid_newtype!(
    /// `inference.stream.SegmentId`: identifies an [`OutputSegment`]. It is declared as the
    /// identifiers of `inference_core::id` are, and like them is written in the canonical
    /// hyphenated form and read only in that form.
    SegmentId
);

/// `inference.stream.OutputSegment.State`: where a segment stands. Serialized as written in ESS
/// (`Tentative`, `Played`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SegmentState {
    /// Generated text that is not yet safe to speak or publish.
    Tentative,
    /// Committed text that may be synthesised and published.
    Committed,
    /// Queued for synthesis and playback.
    Queued,
    /// Its audio is generated and not yet played.
    Synthesized,
    /// The sink acknowledged playback; the user heard it, and it cannot be discarded.
    Played,
    /// Gone and never to be spoken.
    Discarded,
}

impl SegmentState {
    /// Every state, in the order `ess/domains/stream.yaml` lists them.
    pub const ALL: [Self; 6] = [
        Self::Tentative,
        Self::Committed,
        Self::Queued,
        Self::Synthesized,
        Self::Played,
        Self::Discarded,
    ];
}

/// `inference.stream.EmitTextDelta`: the input that adds a Tentative [`OutputSegment`] to the
/// generation `generation_id` names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmitTextDelta {
    pub generation_id: GenerationId,
    pub sequence: i64,
    pub text: String,
}

/// `inference.stream.TextDelta`: tentative text exists; it is not yet safe to speak or publish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDelta {
    pub segment_id: SegmentId,
    pub generation_id: GenerationId,
    pub sequence: i64,
    pub text: String,
}

/// `inference.stream.TextSegmentCommitted`: the segment may be synthesised and published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSegmentCommitted {
    pub segment_id: SegmentId,
    pub text: String,
}

/// `inference.stream.SegmentQueued`: the committed segment is queued for synthesis and playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentQueued {
    pub segment_id: SegmentId,
}

/// `inference.stream.SynthesisCompleted`: the segment's audio is generated and not yet played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SynthesisCompleted {
    pub segment_id: SegmentId,
}

/// `inference.stream.AudioPlayed`: the user heard the segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioPlayed {
    pub segment_id: SegmentId,
}

/// `inference.stream.SegmentDiscarded`: the unplayed segment is gone and will not be spoken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentDiscarded {
    pub segment_id: SegmentId,
}

/// `inference.stream.SegmentStateConflict`: the segment is not in a state the command acts from,
/// so nothing moved. `current` is the state it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentStateConflict {
    pub current: SegmentState,
}

impl fmt::Display for SegmentStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the segment is {:?}, a state this command does not act from, so nothing moved",
            self.current
        )
    }
}

impl Error for SegmentStateConflict {}

/// A refused transition of an [`OutputSegment`]: the segment, handed back unmoved, and the
/// [`SegmentStateConflict`] carrying the state it holds. A transition consumes the segment it
/// moves, so a refused one returns it here; nothing moved.
#[derive(Debug, PartialEq, Eq)]
pub struct SegmentMoveRefused {
    segment: OutputSegment,
    conflict: SegmentStateConflict,
}

impl SegmentMoveRefused {
    /// Why the segment did not move.
    #[must_use]
    pub const fn conflict(&self) -> SegmentStateConflict {
        self.conflict
    }

    /// The segment, as it was before the refused transition.
    #[must_use]
    pub const fn segment(&self) -> &OutputSegment {
        &self.segment
    }

    /// Gives the segment back, as it was before the refused transition.
    #[must_use]
    pub fn into_segment(self) -> OutputSegment {
        self.segment
    }
}

impl From<SegmentMoveRefused> for SegmentStateConflict {
    fn from(refused: SegmentMoveRefused) -> Self {
        refused.conflict
    }
}

impl fmt::Display for SegmentMoveRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.conflict, f)
    }
}

impl Error for SegmentMoveRefused {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.conflict)
    }
}

/// `inference.stream.OutputSegment`: a unit of outbound text from tentative delta to played audio.
/// It belongs to the generation `generation_id` names and carries that generation's `run_id`,
/// copied when the segment was emitted.
///
/// It is not `Clone`: a copy could be offered to a second synthesis and played twice. Its
/// transitions consume it, so moving it leaves no earlier value to move again. A reader that keeps
/// what a segment held, to compare it later, keeps its [`SegmentSnapshot`].
///
/// ```compile_fail
/// fn cloneable<T: Clone>() {}
/// cloneable::<inference_text::OutputSegment>();
/// ```
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct OutputSegment {
    segment_id: SegmentId,
    generation_id: GenerationId,
    run_id: RunId,
    sequence: i64,
    text: String,
    state: SegmentState,
}

impl OutputSegment {
    /// Runs the `emitted` outcome of `inference.stream.EmitTextDelta` on `generation`: generates
    /// a [`SegmentId`] and returns the Tentative segment, carrying the generation's identity and
    /// run, and its [`TextDelta`].
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the generation's state when it is not Active: the
    /// delta is stale and no segment is created.
    pub fn emit(
        generation: &Generation,
        sequence: i64,
        text: String,
    ) -> Result<(Self, TextDelta), GenerationStateConflict> {
        Self::emit_as(SegmentId::generate(), generation, sequence, text)
    }

    /// Runs the `emitted` outcome of `inference.stream.EmitTextDelta` under `segment_id` instead
    /// of a generated one, and otherwise as [`OutputSegment::emit`]. It is for a store that
    /// assigns the identities itself, such as the counter of `inference-testkit`. It accepts any
    /// identity, so a caller that passes one identity twice gets two segments of it; keeping
    /// identities unique is the caller's duty.
    ///
    /// # Errors
    ///
    /// [`GenerationStateConflict`] carrying the generation's state when it is not Active; no
    /// segment is created and `segment_id` is not used.
    pub fn emit_as(
        segment_id: SegmentId,
        generation: &Generation,
        sequence: i64,
        text: String,
    ) -> Result<(Self, TextDelta), GenerationStateConflict> {
        if generation.state() != GenerationState::Active {
            return Err(GenerationStateConflict {
                current: generation.state(),
            });
        }
        let event = TextDelta {
            segment_id,
            generation_id: generation.generation_id(),
            sequence,
            text: text.clone(),
        };
        let segment = Self {
            segment_id,
            generation_id: generation.generation_id(),
            run_id: generation.run_id(),
            sequence,
            text,
            state: SegmentState::Tentative,
        };
        Ok((segment, event))
    }

    /// The segment moved to `to`, if it holds one of the states in `from`, and otherwise handed
    /// back unmoved.
    fn moved(self, from: &[SegmentState], to: SegmentState) -> Result<Self, SegmentMoveRefused> {
        if !from.contains(&self.state) {
            let current = self.state;
            return Err(SegmentMoveRefused {
                segment: self,
                conflict: SegmentStateConflict { current },
            });
        }
        Ok(Self { state: to, ..self })
    }

    /// What the segment holds now, as a value to read and compare. No transition runs on a
    /// snapshot, so it cannot be queued, synthesised or played.
    ///
    /// ```
    /// # use inference_core::generation::{Generation, StartGeneration};
    /// # use inference_core::id::{NodeId, RunId};
    /// # use inference_text::{OutputSegment, SegmentState};
    /// let (generation, _) = Generation::start(StartGeneration {
    ///     run_id: RunId::generate(),
    ///     node_id: NodeId::generate(),
    ///     epoch: 1,
    /// });
    /// let (tentative, _) = OutputSegment::emit(&generation, 1, "hello".to_owned()).unwrap();
    /// let before = tentative.snapshot();
    /// let (committed, _) = tentative.commit().unwrap();
    /// assert_eq!(before.state(), SegmentState::Tentative);
    /// assert_eq!(committed.snapshot().state(), SegmentState::Committed);
    /// assert_eq!(before.segment_id(), committed.segment_id());
    /// assert_eq!(before.text(), committed.text());
    /// assert_ne!(before, committed.snapshot());
    /// ```
    #[must_use]
    pub fn snapshot(&self) -> SegmentSnapshot {
        SegmentSnapshot {
            segment_id: self.segment_id,
            generation_id: self.generation_id,
            run_id: self.run_id,
            sequence: self.sequence,
            text: self.text.clone(),
            state: self.state,
        }
    }

    /// Runs the `committed` outcome of `inference.stream.CommitSegment`: Tentative to Committed.
    ///
    /// It consumes the segment, so a Tentative segment commits once and yields one Committed
    /// value (a second segment of the same identity can only come from
    /// [`OutputSegment::emit_as`]); committing it again does not compile:
    ///
    /// ```compile_fail,E0382
    /// # use inference_core::generation::{Generation, StartGeneration};
    /// # use inference_core::id::{NodeId, RunId};
    /// # use inference_text::OutputSegment;
    /// # let (generation, _) = Generation::start(StartGeneration {
    /// #     run_id: RunId::generate(),
    /// #     node_id: NodeId::generate(),
    /// #     epoch: 1,
    /// # });
    /// let (tentative, _) = OutputSegment::emit(&generation, 1, "hello".to_owned()).unwrap();
    /// let (first, _) = tentative.commit().unwrap();
    /// let (second, _) = tentative.commit().unwrap();
    /// ```
    ///
    /// # Errors
    ///
    /// [`SegmentMoveRefused`] handing the segment back, its conflict carrying the held state,
    /// when the segment is not Tentative.
    pub fn commit(self) -> Result<(Self, TextSegmentCommitted), SegmentMoveRefused> {
        let segment = self.moved(&[SegmentState::Tentative], SegmentState::Committed)?;
        let event = TextSegmentCommitted {
            segment_id: segment.segment_id,
            text: segment.text.clone(),
        };
        Ok((segment, event))
    }

    /// Runs the `queued` outcome of `inference.stream.QueueForSynthesis`: Committed to Queued.
    /// Synthesis consumes committed segments only; the speculative lower-integrity mode is not
    /// declared.
    ///
    /// # Errors
    ///
    /// [`SegmentMoveRefused`] handing the segment back, its conflict carrying the held state,
    /// when the segment is not Committed, a Tentative one among them.
    pub fn queue(self) -> Result<(Self, SegmentQueued), SegmentMoveRefused> {
        let segment = self.moved(&[SegmentState::Committed], SegmentState::Queued)?;
        let event = SegmentQueued {
            segment_id: segment.segment_id,
        };
        Ok((segment, event))
    }

    /// Runs the `synthesized` outcome of `inference.stream.CompleteSynthesis`: Queued to
    /// Synthesized.
    ///
    /// # Errors
    ///
    /// [`SegmentMoveRefused`] handing the segment back, its conflict carrying the held state,
    /// when the segment is not Queued.
    pub fn synthesize(self) -> Result<(Self, SynthesisCompleted), SegmentMoveRefused> {
        let segment = self.moved(&[SegmentState::Queued], SegmentState::Synthesized)?;
        let event = SynthesisCompleted {
            segment_id: segment.segment_id,
        };
        Ok((segment, event))
    }

    /// Runs the `played` outcome of `inference.stream.AcknowledgePlayback`: Synthesized to Played.
    /// Only the media sink confirms playback.
    ///
    /// # Errors
    ///
    /// [`SegmentMoveRefused`] handing the segment back, its conflict carrying the held state,
    /// when the segment is not Synthesized, a Queued one among them.
    pub fn play(self) -> Result<(Self, AudioPlayed), SegmentMoveRefused> {
        let segment = self.moved(&[SegmentState::Synthesized], SegmentState::Played)?;
        let event = AudioPlayed {
            segment_id: segment.segment_id,
        };
        Ok((segment, event))
    }

    /// Runs the `discarded` outcome of `inference.stream.DiscardSegment`: Tentative, Committed,
    /// Queued or Synthesized to Discarded. It is also the move a store makes on every unplayed
    /// segment of a generation that is superseded or cancelled.
    ///
    /// # Errors
    ///
    /// [`SegmentMoveRefused`] handing the segment back, its conflict carrying the held state,
    /// when the segment is Played, which cannot be unsaid, or already Discarded.
    pub fn discard(self) -> Result<(Self, SegmentDiscarded), SegmentMoveRefused> {
        let segment = self.moved(
            &[
                SegmentState::Tentative,
                SegmentState::Committed,
                SegmentState::Queued,
                SegmentState::Synthesized,
            ],
            SegmentState::Discarded,
        )?;
        let event = SegmentDiscarded {
            segment_id: segment.segment_id,
        };
        Ok((segment, event))
    }

    #[must_use]
    pub const fn segment_id(&self) -> SegmentId {
        self.segment_id
    }

    #[must_use]
    pub const fn generation_id(&self) -> GenerationId {
        self.generation_id
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn sequence(&self) -> i64 {
        self.sequence
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn state(&self) -> SegmentState {
        self.state
    }
}

/// What an [`OutputSegment`] held at one moment, made by [`OutputSegment::snapshot`]. It is a
/// value to read and compare, not a segment: no transition runs on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentSnapshot {
    segment_id: SegmentId,
    generation_id: GenerationId,
    run_id: RunId,
    sequence: i64,
    text: String,
    state: SegmentState,
}

impl SegmentSnapshot {
    #[must_use]
    pub const fn segment_id(&self) -> SegmentId {
        self.segment_id
    }

    #[must_use]
    pub const fn generation_id(&self) -> GenerationId {
        self.generation_id
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn sequence(&self) -> i64 {
        self.sequence
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub const fn state(&self) -> SegmentState {
        self.state
    }
}
