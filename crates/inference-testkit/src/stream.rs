//! The turns, spans, generations and output segments of `ess/domains/stream.yaml`, held in
//! memory, and the log that records their events in envelopes.
//!
//! [`StreamDomain`] holds one store per entity, [`TurnStore`], [`SpanStore`], [`GenerationStore`]
//! and [`SegmentStore`], and records every event it emits in order. Its commands run the
//! transitions of `inference-text`'s [`Turn`] and [`Span`], `inference-core`'s [`Generation`] and
//! `inference-text`'s [`OutputSegment`] under identities the domain issues from one counter
//! rendered as a Uuid, `00000000-0000-4000-8000-000000000001` first, so that two runs of the same
//! commands report the same identities. What only a store can decide is decided here: a command
//! naming an identity no record carries is refused before anything moves, a hypothesis names a
//! stored Open turn, completing a turn moves none of its spans, and `SupersedeGeneration` and
//! `CancelGeneration` discard every unplayed segment of the generation in the same outcome,
//! leaving a Played one as it is. The cascade emits no event of its own: the outcome emits its
//! generation event only, as `ess/domains/stream.yaml` declares it. The views
//! `inference.stream.Turns`, `inference.stream.Spans`, `inference.stream.Generations` and
//! `inference.stream.Segments` are [`StreamDomain::turns_view`], [`StreamDomain::spans_view`],
//! [`StreamDomain::generations_view`] and [`StreamDomain::segments_view`]; all are
//! `read_your_writes`, and every row is read from the stores at the call.
//!
//! `OpenTurn`, `EmitHypothesis` and `StartGeneration` refuse a run no store holds and a run that
//! is not Active, and `StartGeneration` a node no pipeline declares. Those are reads of the
//! `inference.graph` stores, which do not exist yet: [`StreamDomain::open_turn`] and
//! [`StreamDomain::start_generation`] run their one accepting outcome only, and
//! [`StreamDomain::emit_hypothesis`] decides the turn refusals and leaves the run refusals to a
//! caller that has decided them against those stores.
//!
//! [`EventLog`] is §23.1 test 6 of the design: it records `inference.stream` events in their
//! [`EventEnvelope`] and refuses a second event carrying an `event_id` already recorded, an event
//! whose `sequence` is not greater than the last one recorded on its `stream_id`, an event whose
//! `kind` is not the declared name of its payload's event, and an event whose payload carries a
//! `generation_id` while the envelope carries none or a different one. It takes envelopes its
//! caller built: the graph runtime, which knows the producing node, its port and the stream it
//! orders its events on, is the producer of envelopes, and no store knows those three.
//!
//! [`StreamDomain`]: crate::stream::StreamDomain
//! [`TurnStore`]: crate::stream::TurnStore
//! [`SpanStore`]: crate::stream::SpanStore
//! [`GenerationStore`]: crate::stream::GenerationStore
//! [`SegmentStore`]: crate::stream::SegmentStore
//! [`EventLog`]: crate::stream::EventLog
//! [`EventEnvelope`]: inference_core::envelope::EventEnvelope
//! [`Turn`]: inference_text::Turn
//! [`Span`]: inference_text::Span
//! [`Generation`]: inference_core::generation::Generation
//! [`OutputSegment`]: inference_text::OutputSegment
//! [`StreamDomain::turns_view`]: crate::stream::StreamDomain::turns_view
//! [`StreamDomain::spans_view`]: crate::stream::StreamDomain::spans_view
//! [`StreamDomain::generations_view`]: crate::stream::StreamDomain::generations_view
//! [`StreamDomain::segments_view`]: crate::stream::StreamDomain::segments_view
//! [`StreamDomain::open_turn`]: crate::stream::StreamDomain::open_turn
//! [`StreamDomain::emit_hypothesis`]: crate::stream::StreamDomain::emit_hypothesis
//! [`StreamDomain::start_generation`]: crate::stream::StreamDomain::start_generation

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use uuid::Uuid;

use inference_core::envelope::{EventEnvelope, EventId};
use inference_core::generation::{
    Generation, GenerationCancelled, GenerationCompleted, GenerationNotFound, GenerationStarted,
    GenerationState, GenerationStateConflict, GenerationSuperseded, StartGeneration,
};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_text::{
    AudioPlayed, CommittedSpanImmutable, EmitHypothesis, EmitTextDelta, OpenTurn, OutputSegment,
    ReviseSpan, SegmentDiscarded, SegmentId, SegmentQueued, SegmentState, SegmentStateConflict,
    Span, SpanId, SpanNotFound, SpanState, SpanStateConflict, SynthesisCompleted, TextDelta,
    TextSegmentCommitted, TranscriptCommitted, TranscriptHypothesis, TranscriptRevision, Turn,
    TurnCompleted, TurnId, TurnNotFound, TurnOpened, TurnState, TurnStateConflict,
};

/// The store of `inference.stream.Turn`, in the order the turns were opened.
#[derive(Debug, Clone, Default)]
pub struct TurnStore {
    turns: Vec<Turn>,
}

impl TurnStore {
    /// The turn carrying `turn_id`, if one was opened.
    #[must_use]
    pub fn get(&self, turn_id: TurnId) -> Option<&Turn> {
        self.turns.iter().find(|turn| turn.turn_id() == turn_id)
    }

    /// Every turn, in the order they were opened.
    pub fn iter(&self) -> impl Iterator<Item = &Turn> {
        self.turns.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }
}

/// The store of `inference.stream.Span`, in the order the spans were emitted. A span stays when
/// its turn completes: a committed span is durable transcript history.
#[derive(Debug, Clone, Default)]
pub struct SpanStore {
    spans: Vec<Span>,
}

impl SpanStore {
    /// The span carrying `span_id`, if one was emitted.
    #[must_use]
    pub fn get(&self, span_id: SpanId) -> Option<&Span> {
        self.spans.iter().find(|span| span.span_id() == span_id)
    }

    /// Every span, in the order they were emitted.
    pub fn iter(&self) -> impl Iterator<Item = &Span> {
        self.spans.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

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

/// An event of `inference.stream`, as the domain emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// `inference.stream.TurnOpened`.
    TurnOpened(TurnOpened),
    /// `inference.stream.TurnCompleted`.
    TurnCompleted(TurnCompleted),
    /// `inference.stream.TranscriptHypothesis`.
    TranscriptHypothesis(TranscriptHypothesis),
    /// `inference.stream.TranscriptRevision`.
    TranscriptRevision(TranscriptRevision),
    /// `inference.stream.TranscriptCommitted`.
    TranscriptCommitted(TranscriptCommitted),
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

impl StreamEvent {
    /// The name `ess/domains/stream.yaml` declares the event under, `inference.stream.TextDelta`
    /// for a [`StreamEvent::TextDelta`]: the `kind` of its envelope.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::TurnOpened(_) => "inference.stream.TurnOpened",
            Self::TurnCompleted(_) => "inference.stream.TurnCompleted",
            Self::TranscriptHypothesis(_) => "inference.stream.TranscriptHypothesis",
            Self::TranscriptRevision(_) => "inference.stream.TranscriptRevision",
            Self::TranscriptCommitted(_) => "inference.stream.TranscriptCommitted",
            Self::GenerationStarted(_) => "inference.stream.GenerationStarted",
            Self::GenerationCompleted(_) => "inference.stream.GenerationCompleted",
            Self::GenerationSuperseded(_) => "inference.stream.GenerationSuperseded",
            Self::GenerationCancelled(_) => "inference.stream.GenerationCancelled",
            Self::TextDelta(_) => "inference.stream.TextDelta",
            Self::TextSegmentCommitted(_) => "inference.stream.TextSegmentCommitted",
            Self::SegmentQueued(_) => "inference.stream.SegmentQueued",
            Self::SynthesisCompleted(_) => "inference.stream.SynthesisCompleted",
            Self::AudioPlayed(_) => "inference.stream.AudioPlayed",
            Self::SegmentDiscarded(_) => "inference.stream.SegmentDiscarded",
        }
    }

    /// The `generation_id` the payload carries: the generation events' and `TextDelta`'s, the
    /// five events whose declaration has the field, and none for every other event.
    #[must_use]
    pub const fn generation_id(&self) -> Option<GenerationId> {
        match self {
            Self::GenerationStarted(event) => Some(event.generation_id),
            Self::GenerationCompleted(event) => Some(event.generation_id),
            Self::GenerationSuperseded(event) => Some(event.generation_id),
            Self::GenerationCancelled(event) => Some(event.generation_id),
            Self::TextDelta(event) => Some(event.generation_id),
            Self::TurnOpened(_)
            | Self::TurnCompleted(_)
            | Self::TranscriptHypothesis(_)
            | Self::TranscriptRevision(_)
            | Self::TranscriptCommitted(_)
            | Self::TextSegmentCommitted(_)
            | Self::SegmentQueued(_)
            | Self::SynthesisCompleted(_)
            | Self::AudioPlayed(_)
            | Self::SegmentDiscarded(_) => None,
        }
    }
}

/// Why [`EventLog::record`] recorded nothing: one rule of §23.1 test 6 per variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeRefusal {
    /// An event carrying `event_id` is already recorded.
    DuplicateEventId { event_id: EventId },
    /// The event's `sequence` is not greater than `last`, the last one recorded on `stream_id`.
    SequenceNotIncreasing {
        stream_id: StreamId,
        last: i64,
        sequence: i64,
    },
    /// The envelope's `kind` is not `payload`, the declared name of its payload's event, so a
    /// consumer dispatching on `kind` would read the event as another, a control event as user
    /// content among them.
    KindMismatch { kind: String, payload: &'static str },
    /// The payload carries the generation `payload`, and the envelope `envelope`, which is none
    /// or another.
    GenerationMismatch {
        payload: GenerationId,
        envelope: Option<GenerationId>,
    },
}

impl fmt::Display for EnvelopeRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateEventId { event_id } => {
                write!(
                    f,
                    "an event carrying the identity {event_id} is already recorded"
                )
            }
            Self::SequenceNotIncreasing {
                stream_id,
                last,
                sequence,
            } => write!(
                f,
                "sequence {sequence} on the stream {stream_id} is not greater than its last, {last}"
            ),
            Self::GenerationMismatch {
                payload,
                envelope: Some(envelope),
            } => write!(
                f,
                "the payload carries the generation {payload} and the envelope {envelope}"
            ),
            Self::GenerationMismatch {
                payload,
                envelope: None,
            } => write!(
                f,
                "the payload carries the generation {payload} and the envelope none"
            ),
            Self::KindMismatch { kind, payload } => write!(
                f,
                "the envelope's kind is `{kind}` and its payload is a `{payload}`"
            ),
        }
    }
}

impl Error for EnvelopeRefusal {}

/// The log of `inference.stream` events, each recorded in its [`EventEnvelope`], in the order
/// they were recorded. [`EventLog::record`] enforces the event ids, generation ids and per-stream
/// sequence of §23.1 test 6 (`docs/design/inference.md:1110`), and that `kind` names the
/// payload's event (line 297); a refused event is not recorded. It takes envelopes its caller
/// built: the graph runtime, which knows the producing node, its port and its stream, is the
/// producer of envelopes, and [`StreamDomain`] records its events without them.
#[derive(Debug, Clone, Default)]
pub struct EventLog {
    envelopes: Vec<EventEnvelope<StreamEvent>>,
    event_ids: BTreeSet<EventId>,
    last_sequence: BTreeMap<StreamId, i64>,
}

impl EventLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `envelope` after the ones recorded before it.
    ///
    /// # Errors
    ///
    /// [`EnvelopeRefusal::DuplicateEventId`] when an event carrying its `event_id` is recorded,
    /// [`EnvelopeRefusal::SequenceNotIncreasing`] when its `sequence` is not greater than the last
    /// one recorded on its `stream_id`, [`EnvelopeRefusal::KindMismatch`] when its `kind` is not
    /// the declared name of its payload's event, and [`EnvelopeRefusal::GenerationMismatch`] when
    /// its payload carries a `generation_id` and the envelope carries none or another. The checks
    /// run in that order, event id, sequence, kind, generation, the first that fails is the
    /// answer, and a refused event is not recorded.
    pub fn record(&mut self, envelope: EventEnvelope<StreamEvent>) -> Result<(), EnvelopeRefusal> {
        if self.event_ids.contains(&envelope.event_id) {
            return Err(EnvelopeRefusal::DuplicateEventId {
                event_id: envelope.event_id,
            });
        }
        if let Some(&last) = self.last_sequence.get(&envelope.stream_id)
            && envelope.sequence <= last
        {
            return Err(EnvelopeRefusal::SequenceNotIncreasing {
                stream_id: envelope.stream_id,
                last,
                sequence: envelope.sequence,
            });
        }
        if envelope.kind != envelope.payload.name() {
            return Err(EnvelopeRefusal::KindMismatch {
                kind: envelope.kind,
                payload: envelope.payload.name(),
            });
        }
        if let Some(payload) = envelope.payload.generation_id()
            && envelope.generation_id != Some(payload)
        {
            return Err(EnvelopeRefusal::GenerationMismatch {
                payload,
                envelope: envelope.generation_id,
            });
        }
        self.event_ids.insert(envelope.event_id);
        self.last_sequence
            .insert(envelope.stream_id, envelope.sequence);
        self.envelopes.push(envelope);
        Ok(())
    }

    /// Every recorded envelope, in the order it was recorded.
    #[must_use]
    pub fn envelopes(&self) -> &[EventEnvelope<StreamEvent>] {
        &self.envelopes
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.envelopes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.envelopes.is_empty()
    }
}

/// Why `inference.stream.CompleteTurn` moved nothing. Both are the command's `wrong-state`
/// outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnMoveError {
    /// No stored turn carries the identity sent; there is no state to report.
    Unknown(TurnNotFound),
    /// The turn is not Open.
    Conflict(TurnStateConflict),
}

impl fmt::Display for TurnMoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(error) => fmt::Display::fmt(error, f),
            Self::Conflict(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for TurnMoveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unknown(error) => Some(error),
            Self::Conflict(error) => Some(error),
        }
    }
}

/// Why `inference.stream.EmitHypothesis` created no span, of the refusals a store holding the
/// turns decides. The run refusals are the caller's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitHypothesisError {
    /// The `unknown-turn` outcome: no stored turn carries the identity sent.
    UnknownTurn(TurnNotFound),
    /// The `turn-not-open` outcome: the turn is Completed.
    TurnNotOpen(TurnStateConflict),
}

impl fmt::Display for EmitHypothesisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTurn(error) => fmt::Display::fmt(error, f),
            Self::TurnNotOpen(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for EmitHypothesisError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownTurn(error) => Some(error),
            Self::TurnNotOpen(error) => Some(error),
        }
    }
}

/// Why `inference.stream.ReviseSpan` revised nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviseSpanError {
    /// The `no-such-span` outcome: no stored span carries the identity sent.
    NoSuchSpan(SpanNotFound),
    /// The `immutable` outcome: the span is Committed, and its text is unchanged.
    Immutable(CommittedSpanImmutable),
}

impl fmt::Display for ReviseSpanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchSpan(error) => fmt::Display::fmt(error, f),
            Self::Immutable(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ReviseSpanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NoSuchSpan(error) => Some(error),
            Self::Immutable(error) => Some(error),
        }
    }
}

/// Why `inference.stream.CommitSpan` moved nothing. Both are the command's `wrong-state` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanMoveError {
    /// No stored span carries the identity sent; there is no state to report.
    Unknown(SpanNotFound),
    /// The span is not a Hypothesis.
    Conflict(SpanStateConflict),
}

impl fmt::Display for SpanMoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(error) => fmt::Display::fmt(error, f),
            Self::Conflict(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for SpanMoveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unknown(error) => Some(error),
            Self::Conflict(error) => Some(error),
        }
    }
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

/// A row of the view `inference.stream.Turns`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnsRow {
    pub turn_id: TurnId,
    pub run_id: RunId,
    pub state: TurnState,
}

/// A row of the view `inference.stream.Spans`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpansRow {
    pub span_id: SpanId,
    pub run_id: RunId,
    pub turn_id: TurnId,
    pub stream_id: StreamId,
    pub sequence: i64,
    pub text: String,
    pub state: SpanState,
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

/// `inference.stream` in memory: its four entity stores and the events emitted.
#[derive(Debug, Clone, Default)]
pub struct StreamDomain {
    turns: TurnStore,
    spans: SpanStore,
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

    /// Runs the `opened` outcome of `inference.stream.OpenTurn`: stores the Open turn
    /// [`Turn::open_as`] returns and records its [`TurnOpened`]. The run refusals are the
    /// caller's, decided before it calls this.
    pub fn open_turn(&mut self, input: OpenTurn) -> TurnOpened {
        let turn_id = TurnId::from(self.issue());
        let (turn, event) = Turn::open_as(turn_id, input);
        self.turns.turns.push(turn);
        self.events.push(StreamEvent::TurnOpened(event));
        event
    }

    /// Runs `inference.stream.CompleteTurn`: the turn moves to Completed and none of its spans
    /// moves.
    ///
    /// # Errors
    ///
    /// [`TurnMoveError::Unknown`] when no stored turn carries `turn_id`,
    /// [`TurnMoveError::Conflict`] when it is not Open. Nothing moves and no event is recorded.
    pub fn complete_turn(&mut self, turn_id: TurnId) -> Result<TurnCompleted, TurnMoveError> {
        let slot = self
            .turns
            .turns
            .iter_mut()
            .find(|turn| turn.turn_id() == turn_id)
            .ok_or(TurnMoveError::Unknown(TurnNotFound { turn_id }))?;
        let (moved, event) = slot.complete().map_err(TurnMoveError::Conflict)?;
        *slot = moved;
        self.events.push(StreamEvent::TurnCompleted(event));
        Ok(event)
    }

    /// Runs `inference.stream.EmitHypothesis` past its run refusals, which are the caller's:
    /// stores the Hypothesis span [`Span::emit_as`] returns and records its
    /// [`TranscriptHypothesis`].
    ///
    /// # Errors
    ///
    /// [`EmitHypothesisError::UnknownTurn`] when no stored turn carries `turn_id`, otherwise
    /// [`EmitHypothesisError::TurnNotOpen`] when it is Completed. Nothing is stored, no event is
    /// recorded and no identity is issued.
    pub fn emit_hypothesis(
        &mut self,
        input: EmitHypothesis,
    ) -> Result<TranscriptHypothesis, EmitHypothesisError> {
        let turn = self
            .turns
            .get(input.turn_id)
            .ok_or(EmitHypothesisError::UnknownTurn(TurnNotFound {
                turn_id: input.turn_id,
            }))?;
        let span_id = SpanId::from(Self::identity(self.issued + 1));
        let (span, event) = Span::emit_as(
            span_id,
            turn,
            input.run_id,
            input.stream_id,
            input.sequence,
            input.text,
        )
        .map_err(EmitHypothesisError::TurnNotOpen)?;
        self.issue();
        self.spans.spans.push(span);
        self.events
            .push(StreamEvent::TranscriptHypothesis(event.clone()));
        Ok(event)
    }

    /// Runs `inference.stream.ReviseSpan`: the Hypothesis now reads the revised text.
    ///
    /// # Errors
    ///
    /// [`ReviseSpanError::NoSuchSpan`] when no stored span carries `span_id`,
    /// [`ReviseSpanError::Immutable`] when it is Committed. The text is unchanged and no event is
    /// recorded.
    pub fn revise_span(
        &mut self,
        input: ReviseSpan,
    ) -> Result<TranscriptRevision, ReviseSpanError> {
        let slot = self
            .spans
            .spans
            .iter_mut()
            .find(|span| span.span_id() == input.span_id)
            .ok_or(ReviseSpanError::NoSuchSpan(SpanNotFound {
                span_id: input.span_id,
            }))?;
        let (moved, event) = slot
            .revise(input.text)
            .map_err(ReviseSpanError::Immutable)?;
        *slot = moved;
        self.events
            .push(StreamEvent::TranscriptRevision(event.clone()));
        Ok(event)
    }

    /// Runs `inference.stream.CommitSpan`.
    ///
    /// # Errors
    ///
    /// [`SpanMoveError::Unknown`] when no stored span carries `span_id`,
    /// [`SpanMoveError::Conflict`] when it is not a Hypothesis.
    pub fn commit_span(&mut self, span_id: SpanId) -> Result<TranscriptCommitted, SpanMoveError> {
        let slot = self
            .spans
            .spans
            .iter_mut()
            .find(|span| span.span_id() == span_id)
            .ok_or(SpanMoveError::Unknown(SpanNotFound { span_id }))?;
        let (moved, event) = slot.commit().map_err(SpanMoveError::Conflict)?;
        *slot = moved;
        self.events
            .push(StreamEvent::TranscriptCommitted(event.clone()));
        Ok(event)
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

    /// The opened turns.
    #[must_use]
    pub const fn turns(&self) -> &TurnStore {
        &self.turns
    }

    /// The emitted spans.
    #[must_use]
    pub const fn spans(&self) -> &SpanStore {
        &self.spans
    }

    /// The view `inference.stream.Turns`: one row per turn, in the order they were opened.
    #[must_use]
    pub fn turns_view(&self) -> Vec<TurnsRow> {
        self.turns
            .iter()
            .map(|turn| TurnsRow {
                turn_id: turn.turn_id(),
                run_id: turn.run_id(),
                state: turn.state(),
            })
            .collect()
    }

    /// The view `inference.stream.Spans`: one row per span, in the order they were emitted.
    #[must_use]
    pub fn spans_view(&self) -> Vec<SpansRow> {
        self.spans
            .iter()
            .map(|span| SpansRow {
                span_id: span.span_id(),
                run_id: span.run_id(),
                turn_id: span.turn_id(),
                stream_id: span.stream_id(),
                sequence: span.sequence(),
                text: span.text().to_owned(),
                state: span.state(),
            })
            .collect()
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
    use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
    use inference_text::{
        CommittedSpanImmutable, EmitHypothesis, EmitTextDelta, OpenTurn, ReviseSpan, SegmentId,
        SegmentState, SegmentStateConflict, SpanId, SpanNotFound, SpanState, SpanStateConflict,
        TurnId, TurnNotFound, TurnState, TurnStateConflict,
    };
    use uuid::Uuid;

    use super::{
        EmitHypothesisError, EmitTextDeltaError, GenerationMoveError, GenerationsRow,
        ReviseSpanError, SegmentMoveError, SegmentsRow, SpanMoveError, SpansRow, StreamDomain,
        StreamEvent, TurnMoveError, TurnsRow,
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

    fn open(domain: &mut StreamDomain) -> TurnId {
        domain.open_turn(OpenTurn { run_id: run_id() }).turn_id
    }

    fn hypothesis(turn_id: TurnId, sequence: i64) -> EmitHypothesis {
        EmitHypothesis {
            run_id: run_id(),
            turn_id,
            stream_id: stream_id(),
            sequence,
            text: format!("words {sequence}"),
        }
    }

    fn stream_id() -> StreamId {
        StreamId::from(Uuid::from_u128(0x2a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d))
    }

    #[test]
    fn turns_and_spans_take_their_identities_from_the_one_counter() {
        let mut domain = StreamDomain::new();
        let generation = start(&mut domain);
        let turn = open(&mut domain);
        let span = domain
            .emit_hypothesis(hypothesis(turn, 1))
            .expect("an Open turn admits a span")
            .span_id;
        assert_eq!(
            generation.to_string(),
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(turn.to_string(), "00000000-0000-4000-8000-000000000002");
        assert_eq!(span.to_string(), "00000000-0000-4000-8000-000000000003");
    }

    #[test]
    fn a_hypothesis_names_a_stored_open_turn_or_is_refused_and_takes_no_identity() {
        let mut domain = StreamDomain::new();
        let unknown = TurnId::from(Uuid::from_u128(0xff));
        assert_eq!(
            domain.emit_hypothesis(hypothesis(unknown, 1)),
            Err(EmitHypothesisError::UnknownTurn(TurnNotFound {
                turn_id: unknown
            }))
        );

        let turn = open(&mut domain);
        domain.complete_turn(turn).expect("an Open turn completes");
        let events = domain.events().len();
        assert_eq!(
            domain.emit_hypothesis(hypothesis(turn, 1)),
            Err(EmitHypothesisError::TurnNotOpen(TurnStateConflict {
                current: TurnState::Completed
            }))
        );
        assert!(domain.spans().is_empty());
        assert_eq!(domain.events().len(), events);

        let next = open(&mut domain);
        assert_eq!(
            next.to_string(),
            "00000000-0000-4000-8000-000000000002",
            "a refused hypothesis took no identity"
        );
    }

    #[test]
    fn span_commands_on_an_unknown_or_committed_span_move_nothing() {
        let mut domain = StreamDomain::new();
        let unknown = SpanId::from(Uuid::from_u128(0xff));
        assert_eq!(
            domain.commit_span(unknown),
            Err(SpanMoveError::Unknown(SpanNotFound { span_id: unknown }))
        );
        assert_eq!(
            domain.revise_span(ReviseSpan {
                span_id: unknown,
                text: "lost".to_owned()
            }),
            Err(ReviseSpanError::NoSuchSpan(SpanNotFound {
                span_id: unknown
            }))
        );
        assert_eq!(
            domain.complete_turn(TurnId::from(Uuid::from_u128(0xfe))),
            Err(TurnMoveError::Unknown(TurnNotFound {
                turn_id: TurnId::from(Uuid::from_u128(0xfe))
            }))
        );
        assert!(domain.events().is_empty());

        let turn = open(&mut domain);
        let span = domain
            .emit_hypothesis(hypothesis(turn, 1))
            .expect("admitted")
            .span_id;
        domain.commit_span(span).expect("a Hypothesis commits");
        let events = domain.events().len();
        assert_eq!(
            domain.revise_span(ReviseSpan {
                span_id: span,
                text: "rewritten".to_owned()
            }),
            Err(ReviseSpanError::Immutable(CommittedSpanImmutable {
                span_id: span
            }))
        );
        assert_eq!(
            domain.commit_span(span),
            Err(SpanMoveError::Conflict(SpanStateConflict {
                current: SpanState::Committed
            }))
        );
        assert_eq!(domain.spans().get(span).expect("stored").text(), "words 1");
        assert_eq!(domain.events().len(), events);
    }

    #[test]
    fn completing_a_turn_moves_no_span_and_the_views_read_both() {
        let mut domain = StreamDomain::new();
        let turn = open(&mut domain);
        let hypothesis_span = domain
            .emit_hypothesis(hypothesis(turn, 1))
            .expect("admitted")
            .span_id;
        let committed_span = domain
            .emit_hypothesis(hypothesis(turn, 2))
            .expect("admitted")
            .span_id;
        domain.commit_span(committed_span).expect("commits");
        let event = domain.complete_turn(turn).expect("an Open turn completes");
        assert_eq!(event.turn_id, turn);
        assert_eq!(
            domain.events().last(),
            Some(&StreamEvent::TurnCompleted(event))
        );

        assert_eq!(
            domain.turns_view(),
            [TurnsRow {
                turn_id: turn,
                run_id: run_id(),
                state: TurnState::Completed,
            }]
        );
        assert_eq!(
            domain.spans_view(),
            [
                SpansRow {
                    span_id: hypothesis_span,
                    run_id: run_id(),
                    turn_id: turn,
                    stream_id: stream_id(),
                    sequence: 1,
                    text: "words 1".to_owned(),
                    state: SpanState::Hypothesis,
                },
                SpansRow {
                    span_id: committed_span,
                    run_id: run_id(),
                    turn_id: turn,
                    stream_id: stream_id(),
                    sequence: 2,
                    text: "words 2".to_owned(),
                    state: SpanState::Committed,
                },
            ]
        );
    }

    #[test]
    fn the_log_refuses_a_kind_that_is_not_the_payloads_name_after_the_sequence_check() {
        use inference_core::envelope::{EventEnvelope, EventId, Timebase};
        use inference_text::TurnCompleted;

        use super::{EnvelopeRefusal, EventLog};

        let envelope = |event: u128, sequence: i64, kind: &str| EventEnvelope {
            schema_version: 1,
            event_id: EventId::from(Uuid::from_u128(event)),
            run_id: run_id(),
            session_id: None,
            stream_id: stream_id(),
            sequence,
            producer: node_id(),
            port: "control".to_owned(),
            kind: kind.to_owned(),
            generation_id: Some(GenerationId::from(Uuid::from_u128(0x77))),
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: StreamEvent::TurnCompleted(TurnCompleted {
                turn_id: TurnId::from(Uuid::from_u128(0x99)),
            }),
        };
        let mut log = EventLog::new();
        assert_eq!(
            log.record(envelope(1, 1, "inference.stream.TranscriptHypothesis")),
            Err(EnvelopeRefusal::KindMismatch {
                kind: "inference.stream.TranscriptHypothesis".to_owned(),
                payload: "inference.stream.TurnCompleted",
            })
        );
        assert!(log.is_empty());

        log.record(envelope(2, 2, "inference.stream.TurnCompleted"))
            .expect("the payload's own name is its kind");
        assert_eq!(
            log.record(envelope(3, 2, "inference.stream.TextDelta")),
            Err(EnvelopeRefusal::SequenceNotIncreasing {
                stream_id: stream_id(),
                last: 2,
                sequence: 2,
            }),
            "the sequence is checked before the kind"
        );
        assert_eq!(log.len(), 1);
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
