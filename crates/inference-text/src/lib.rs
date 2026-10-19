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
//! discarded. Each transition is a method that returns the moved segment and the event its command
//! emits, or [`SegmentStateConflict`] carrying the state the segment holds; a refused move changes
//! nothing. The crate depends on `inference-core` only and carries no synthesiser, sink or other
//! concrete implementation. Serialized names are the ESS names.

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

/// `inference.stream.OutputSegment`: a unit of outbound text from tentative delta to played audio.
/// It belongs to the generation `generation_id` names and carries that generation's `run_id`,
/// copied when the segment was emitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    /// assigns the identities itself, such as the counter of `inference-testkit`.
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

    /// The segment moved to `to`, if it holds one of the states in `from`.
    fn moved(&self, from: &[SegmentState], to: SegmentState) -> Result<Self, SegmentStateConflict> {
        if !from.contains(&self.state) {
            return Err(SegmentStateConflict {
                current: self.state,
            });
        }
        Ok(Self {
            state: to,
            ..self.clone()
        })
    }

    /// Runs the `committed` outcome of `inference.stream.CommitSegment`: Tentative to Committed.
    ///
    /// # Errors
    ///
    /// [`SegmentStateConflict`] carrying the held state when the segment is not Tentative.
    pub fn commit(&self) -> Result<(Self, TextSegmentCommitted), SegmentStateConflict> {
        let segment = self.moved(&[SegmentState::Tentative], SegmentState::Committed)?;
        let event = TextSegmentCommitted {
            segment_id: self.segment_id,
            text: self.text.clone(),
        };
        Ok((segment, event))
    }

    /// Runs the `queued` outcome of `inference.stream.QueueForSynthesis`: Committed to Queued.
    /// Synthesis consumes committed segments only; the speculative lower-integrity mode is not
    /// declared.
    ///
    /// # Errors
    ///
    /// [`SegmentStateConflict`] carrying the held state when the segment is not Committed, a
    /// Tentative one among them.
    pub fn queue(&self) -> Result<(Self, SegmentQueued), SegmentStateConflict> {
        let segment = self.moved(&[SegmentState::Committed], SegmentState::Queued)?;
        let event = SegmentQueued {
            segment_id: self.segment_id,
        };
        Ok((segment, event))
    }

    /// Runs the `synthesized` outcome of `inference.stream.CompleteSynthesis`: Queued to
    /// Synthesized.
    ///
    /// # Errors
    ///
    /// [`SegmentStateConflict`] carrying the held state when the segment is not Queued.
    pub fn synthesize(&self) -> Result<(Self, SynthesisCompleted), SegmentStateConflict> {
        let segment = self.moved(&[SegmentState::Queued], SegmentState::Synthesized)?;
        let event = SynthesisCompleted {
            segment_id: self.segment_id,
        };
        Ok((segment, event))
    }

    /// Runs the `played` outcome of `inference.stream.AcknowledgePlayback`: Synthesized to Played.
    /// Only the media sink confirms playback.
    ///
    /// # Errors
    ///
    /// [`SegmentStateConflict`] carrying the held state when the segment is not Synthesized, a
    /// Queued one among them.
    pub fn play(&self) -> Result<(Self, AudioPlayed), SegmentStateConflict> {
        let segment = self.moved(&[SegmentState::Synthesized], SegmentState::Played)?;
        let event = AudioPlayed {
            segment_id: self.segment_id,
        };
        Ok((segment, event))
    }

    /// Runs the `discarded` outcome of `inference.stream.DiscardSegment`: Tentative, Committed,
    /// Queued or Synthesized to Discarded. It is also the move a store makes on every unplayed
    /// segment of a generation that is superseded or cancelled.
    ///
    /// # Errors
    ///
    /// [`SegmentStateConflict`] carrying the held state when the segment is Played, which cannot
    /// be unsaid, or already Discarded.
    pub fn discard(&self) -> Result<(Self, SegmentDiscarded), SegmentStateConflict> {
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
            segment_id: self.segment_id,
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
