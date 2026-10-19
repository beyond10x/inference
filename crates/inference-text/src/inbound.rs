//! The inbound text of `ess/domains/stream.yaml`: a [`Turn`] is the control boundary of one input
//! turn, and a [`Span`] is a unit of inbound text within it.
//!
//! A turn is Open until it is completed; its completion is a control boundary declaring the whole
//! input turn complete, distinct from the commit of its spans, so completing a turn moves no span.
//! A span is a Hypothesis, text subject to later replacement, until it is committed; a revision is
//! an explicit change to a Hypothesis, and a Committed span is immutable, so a revision of one is
//! refused with [`CommittedSpanImmutable`] and its text stays as it was. A stable prefix may be
//! committed before the turn is complete. A completed turn admits no further span. Each transition
//! is a method that returns the moved record and the event its command emits, or the error the
//! command declares; a refused move changes nothing. Which turn or span an identity names, and
//! whether the run a turn or span names exists and is Active, are decided by a store holding them.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use inference_core::id::{RunId, StreamId};

inference_core::uuid_newtype!(
    /// `inference.stream.TurnId`: identifies a [`Turn`]. It is declared as the identifiers of
    /// `inference_core::id` are, and like them is written in the canonical hyphenated form and
    /// read only in that form.
    TurnId
);

inference_core::uuid_newtype!(
    /// `inference.stream.SpanId`: identifies a [`Span`], written and read as [`TurnId`] is.
    SpanId
);

/// `inference.stream.Turn.State`: where a turn stands. Serialized as written in ESS (`Open`,
/// `Completed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TurnState {
    /// The turn admits spans.
    Open,
    /// The whole input turn is declared complete; it admits no further span.
    Completed,
}

impl TurnState {
    /// Every state, in the order `ess/domains/stream.yaml` lists them.
    pub const ALL: [Self; 2] = [Self::Open, Self::Completed];
}

/// `inference.stream.Span.State`: where a span stands. Serialized as written in ESS
/// (`Hypothesis`, `Committed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpanState {
    /// Text subject to later replacement; downstream may speculate on it, side effects may not.
    Hypothesis,
    /// An immutable unit downstream may treat as stable.
    Committed,
}

impl SpanState {
    /// Every state, in the order `ess/domains/stream.yaml` lists them.
    pub const ALL: [Self; 2] = [Self::Hypothesis, Self::Committed];
}

/// `inference.stream.OpenTurn`: the input that opens a [`Turn`] in the run `run_id` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTurn {
    pub run_id: RunId,
}

/// `inference.stream.EmitHypothesis`: the input that adds a Hypothesis [`Span`] to the turn
/// `turn_id` names, at `sequence` on the stream `stream_id` names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmitHypothesis {
    pub run_id: RunId,
    pub turn_id: TurnId,
    pub stream_id: StreamId,
    pub sequence: i64,
    pub text: String,
}

/// `inference.stream.ReviseSpan`: the input that rewrites the text of the Hypothesis `span_id`
/// names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviseSpan {
    pub span_id: SpanId,
    pub text: String,
}

/// `inference.stream.TurnOpened`: an input turn is open and spans may arrive in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnOpened {
    pub turn_id: TurnId,
    pub run_id: RunId,
}

/// `inference.stream.TurnCompleted`: the whole input turn is declared complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnCompleted {
    pub turn_id: TurnId,
}

/// `inference.stream.TranscriptHypothesis`: a provisional span exists; downstream may speculate
/// on it, side effects may not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptHypothesis {
    pub span_id: SpanId,
    pub turn_id: TurnId,
    pub sequence: i64,
    pub text: String,
}

/// `inference.stream.TranscriptRevision`: the hypothesis now reads the revised text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptRevision {
    pub span_id: SpanId,
    pub text: String,
}

/// `inference.stream.TranscriptCommitted`: the span is immutable and safe for side-effecting
/// consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptCommitted {
    pub span_id: SpanId,
    pub text: String,
}

/// `inference.stream.TurnStateConflict`: the turn is not Open, so nothing moved. `current` is the
/// state it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStateConflict {
    pub current: TurnState,
}

impl fmt::Display for TurnStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the turn is {:?}, not Open, so nothing moved",
            self.current
        )
    }
}

impl Error for TurnStateConflict {}

/// `inference.stream.TurnNotFound`: no turn carries the identity a command named. `turn_id` is
/// the identity that was sent. Only a store holding the turns can decide it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnNotFound {
    pub turn_id: TurnId,
}

impl fmt::Display for TurnNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no turn carries the identity {}", self.turn_id)
    }
}

impl Error for TurnNotFound {}

/// `inference.stream.CommittedSpanImmutable`: a revision cannot rewrite a committed span.
/// `span_id` is the span that was not revised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedSpanImmutable {
    pub span_id: SpanId,
}

impl fmt::Display for CommittedSpanImmutable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the span {} is committed; a revision cannot rewrite it",
            self.span_id
        )
    }
}

impl Error for CommittedSpanImmutable {}

/// `inference.stream.SpanNotFound`: no span carries the identity a command named. `span_id` is the
/// identity that was sent. Only a store holding the spans can decide it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanNotFound {
    pub span_id: SpanId,
}

impl fmt::Display for SpanNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no span carries the identity {}", self.span_id)
    }
}

impl Error for SpanNotFound {}

/// `inference.stream.SpanStateConflict`: the span is not a Hypothesis, so nothing moved.
/// `current` is the state it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanStateConflict {
    pub current: SpanState,
}

impl fmt::Display for SpanStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the span is {:?}, not a Hypothesis, so nothing moved",
            self.current
        )
    }
}

impl Error for SpanStateConflict {}

/// `inference.stream.Turn`: one input turn; its completion is a control boundary, not a commit.
/// It references its run by `run_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Turn {
    turn_id: TurnId,
    run_id: RunId,
    state: TurnState,
}

impl Turn {
    /// Runs the `opened` outcome of `inference.stream.OpenTurn`: generates a [`TurnId`] and
    /// returns the Open turn and its [`TurnOpened`].
    ///
    /// The refusals of the command, a run no store holds and a run that is not Active, are
    /// decided by a store holding the runs before it calls this.
    #[must_use]
    pub fn open(input: OpenTurn) -> (Self, TurnOpened) {
        Self::open_as(TurnId::generate(), input)
    }

    /// Runs the `opened` outcome of `inference.stream.OpenTurn` under `turn_id` instead of a
    /// generated one, and otherwise as [`Turn::open`]. It is for a store that assigns the
    /// identities itself, such as the counter of `inference-testkit`.
    #[must_use]
    pub fn open_as(turn_id: TurnId, input: OpenTurn) -> (Self, TurnOpened) {
        let event = TurnOpened {
            turn_id,
            run_id: input.run_id,
        };
        let turn = Self {
            turn_id,
            run_id: input.run_id,
            state: TurnState::Open,
        };
        (turn, event)
    }

    /// Runs the `completed` outcome of `inference.stream.CompleteTurn`: Open to Completed. No span
    /// of the turn moves.
    ///
    /// # Errors
    ///
    /// [`TurnStateConflict`] carrying the held state when the turn is not Open.
    pub fn complete(&self) -> Result<(Self, TurnCompleted), TurnStateConflict> {
        if self.state != TurnState::Open {
            return Err(TurnStateConflict {
                current: self.state,
            });
        }
        let turn = Self {
            state: TurnState::Completed,
            ..self.clone()
        };
        let event = TurnCompleted {
            turn_id: self.turn_id,
        };
        Ok((turn, event))
    }

    #[must_use]
    pub const fn turn_id(&self) -> TurnId {
        self.turn_id
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn state(&self) -> TurnState {
        self.state
    }
}

/// `inference.stream.Span`: a unit of inbound text, provisional until committed and immutable
/// after. It references its run by `run_id` and the turn it arrived in by `turn_id`, and carries
/// its `sequence` on the stream `stream_id` names. That `run_id` is its turn's run is not checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Span {
    span_id: SpanId,
    run_id: RunId,
    turn_id: TurnId,
    stream_id: StreamId,
    sequence: i64,
    text: String,
    state: SpanState,
}

impl Span {
    /// Runs the `emitted` outcome of `inference.stream.EmitHypothesis` in `turn`: generates a
    /// [`SpanId`] and returns the Hypothesis span, carrying the turn's identity, and its
    /// [`TranscriptHypothesis`].
    ///
    /// # Errors
    ///
    /// [`TurnStateConflict`] carrying the turn's state when it is not Open: a completed turn
    /// admits no further span, and none is created.
    pub fn emit(
        turn: &Turn,
        run_id: RunId,
        stream_id: StreamId,
        sequence: i64,
        text: String,
    ) -> Result<(Self, TranscriptHypothesis), TurnStateConflict> {
        Self::emit_as(SpanId::generate(), turn, run_id, stream_id, sequence, text)
    }

    /// Runs the `emitted` outcome of `inference.stream.EmitHypothesis` under `span_id` instead of
    /// a generated one, and otherwise as [`Span::emit`]. It is for a store that assigns the
    /// identities itself, such as the counter of `inference-testkit`.
    ///
    /// # Errors
    ///
    /// [`TurnStateConflict`] carrying the turn's state when it is not Open; no span is created
    /// and `span_id` is not used.
    pub fn emit_as(
        span_id: SpanId,
        turn: &Turn,
        run_id: RunId,
        stream_id: StreamId,
        sequence: i64,
        text: String,
    ) -> Result<(Self, TranscriptHypothesis), TurnStateConflict> {
        if turn.state() != TurnState::Open {
            return Err(TurnStateConflict {
                current: turn.state(),
            });
        }
        let event = TranscriptHypothesis {
            span_id,
            turn_id: turn.turn_id(),
            sequence,
            text: text.clone(),
        };
        let span = Self {
            span_id,
            run_id,
            turn_id: turn.turn_id(),
            stream_id,
            sequence,
            text,
            state: SpanState::Hypothesis,
        };
        Ok((span, event))
    }

    /// Runs the `revised` outcome of `inference.stream.ReviseSpan`: the Hypothesis now reads
    /// `text` and stays a Hypothesis.
    ///
    /// # Errors
    ///
    /// [`CommittedSpanImmutable`] carrying the span's identity when it is Committed: the revision
    /// is refused and the text is unchanged.
    pub fn revise(
        &self,
        text: String,
    ) -> Result<(Self, TranscriptRevision), CommittedSpanImmutable> {
        if self.state != SpanState::Hypothesis {
            return Err(CommittedSpanImmutable {
                span_id: self.span_id,
            });
        }
        let event = TranscriptRevision {
            span_id: self.span_id,
            text: text.clone(),
        };
        let span = Self {
            text,
            ..self.clone()
        };
        Ok((span, event))
    }

    /// Runs the `committed` outcome of `inference.stream.CommitSpan`: Hypothesis to Committed.
    ///
    /// # Errors
    ///
    /// [`SpanStateConflict`] carrying the held state when the span is not a Hypothesis.
    pub fn commit(&self) -> Result<(Self, TranscriptCommitted), SpanStateConflict> {
        if self.state != SpanState::Hypothesis {
            return Err(SpanStateConflict {
                current: self.state,
            });
        }
        let span = Self {
            state: SpanState::Committed,
            ..self.clone()
        };
        let event = TranscriptCommitted {
            span_id: self.span_id,
            text: self.text.clone(),
        };
        Ok((span, event))
    }

    #[must_use]
    pub const fn span_id(&self) -> SpanId {
        self.span_id
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn turn_id(&self) -> TurnId {
        self.turn_id
    }

    #[must_use]
    pub const fn stream_id(&self) -> StreamId {
        self.stream_id
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
    pub const fn state(&self) -> SpanState {
        self.state
    }
}
