//! The `inference.stream` events the model nodes emit ([`StreamEvent`]), the log that records them
//! in their envelopes ([`EventLog`]), and the log of one run its nodes share ([`RunLog`]).
//!
//! [`EventLog`] is §23.1 test 6 of the design: it records `inference.stream` events in their
//! [`EventEnvelope`] and refuses a second event carrying an `event_id` already recorded, an event
//! whose `sequence` is not greater than the last one recorded on its `stream_id`, an event whose
//! `kind` is not the declared name of its payload's event, and an event whose payload carries a
//! `generation_id` while the envelope carries none or a different one. It takes envelopes its
//! caller built: the graph runtime, which knows the producing node, its port and the stream it
//! orders its events on, is the producer of envelopes, and no store knows those three.
//!
//! A node wraps what it emits on one output port with a `StreamEmitter`: one stream of its own,
//! numbered from 1, every `inference.stream` event recorded in the run's [`RunLog`] before it is
//! sent, and a control event, told from user content by its envelope's `kind` alone, not
//! recorded.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use inference_core::envelope::{EventEnvelope, EventId};
use inference_core::generation::{
    GenerationCancelled, GenerationCompleted, GenerationRefused, GenerationStarted,
    GenerationSuperseded,
};
use inference_core::id::{GenerationId, StreamId};
use inference_text::{
    AudioPlayed, SegmentDiscarded, SegmentQueued, SynthesisCompleted, TextDelta,
    TextSegmentCommitted, TranscriptCommitted, TranscriptHypothesis, TranscriptRevision,
    TurnCompleted, TurnOpened,
};

use super::vad::{SpeechEnded, SpeechStarted};

/// An event of `inference.stream`, as the domain emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// `inference.stream.SpeechStarted`.
    SpeechStarted(SpeechStarted),
    /// `inference.stream.SpeechEnded`.
    SpeechEnded(SpeechEnded),
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
    /// `inference.stream.GenerationRefused`.
    GenerationRefused(GenerationRefused),
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
            Self::SpeechStarted(_) => "inference.stream.SpeechStarted",
            Self::SpeechEnded(_) => "inference.stream.SpeechEnded",
            Self::TurnOpened(_) => "inference.stream.TurnOpened",
            Self::TurnCompleted(_) => "inference.stream.TurnCompleted",
            Self::TranscriptHypothesis(_) => "inference.stream.TranscriptHypothesis",
            Self::TranscriptRevision(_) => "inference.stream.TranscriptRevision",
            Self::TranscriptCommitted(_) => "inference.stream.TranscriptCommitted",
            Self::GenerationStarted(_) => "inference.stream.GenerationStarted",
            Self::GenerationCompleted(_) => "inference.stream.GenerationCompleted",
            Self::GenerationSuperseded(_) => "inference.stream.GenerationSuperseded",
            Self::GenerationCancelled(_) => "inference.stream.GenerationCancelled",
            Self::GenerationRefused(_) => "inference.stream.GenerationRefused",
            Self::TextDelta(_) => "inference.stream.TextDelta",
            Self::TextSegmentCommitted(_) => "inference.stream.TextSegmentCommitted",
            Self::SegmentQueued(_) => "inference.stream.SegmentQueued",
            Self::SynthesisCompleted(_) => "inference.stream.SynthesisCompleted",
            Self::AudioPlayed(_) => "inference.stream.AudioPlayed",
            Self::SegmentDiscarded(_) => "inference.stream.SegmentDiscarded",
        }
    }

    /// The `generation_id` the payload carries: the generation events' and `TextDelta`'s, the
    /// six events whose declaration has the field, and none for every other event.
    #[must_use]
    pub const fn generation_id(&self) -> Option<GenerationId> {
        match self {
            Self::GenerationStarted(event) => Some(event.generation_id),
            Self::GenerationCompleted(event) => Some(event.generation_id),
            Self::GenerationSuperseded(event) => Some(event.generation_id),
            Self::GenerationCancelled(event) => Some(event.generation_id),
            Self::GenerationRefused(event) => Some(event.generation_id),
            Self::TextDelta(event) => Some(event.generation_id),
            Self::SpeechStarted(_)
            | Self::SpeechEnded(_)
            | Self::TurnOpened(_)
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
/// producer of envelopes, and a store of the stream domain records its events without them.
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

/// The event log of one run, shared by its nodes and their reader: every `inference.stream` event
/// a node emits, in its envelope, in the order the nodes recorded them. A clone reads and records
/// into the same log.
#[derive(Debug, Clone, Default)]
pub struct RunLog(Arc<Mutex<EventLog>>);

impl RunLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn log(&self) -> MutexGuard<'_, EventLog> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records `envelope` as [`EventLog::record`] does.
    ///
    /// # Errors
    ///
    /// The [`EnvelopeRefusal`] of [`EventLog::record`]; nothing is recorded.
    pub fn record(&self, envelope: EventEnvelope<StreamEvent>) -> Result<(), EnvelopeRefusal> {
        self.log().record(envelope)
    }

    /// Every recorded envelope, in the order it was recorded.
    #[must_use]
    pub fn envelopes(&self) -> Vec<EventEnvelope<StreamEvent>> {
        self.log().envelopes().to_vec()
    }
}

#[cfg(any(feature = "llm", feature = "stt", feature = "tts"))]
pub(crate) use emit::StreamEmitter;

/// What a model node emits, wrapped in envelopes and recorded in its run's [`RunLog`].
#[cfg(any(feature = "llm", feature = "stt", feature = "tts"))]
mod emit {
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
    use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
    use inference_text::SegmentId;

    use super::{EnvelopeRefusal, RunLog, StreamEvent};

    /// Wraps what one node emits on one output port in envelopes: one stream of its own, numbered
    /// from 1. An envelope names the generation its event names; an event naming only its segment,
    /// such as `TextSegmentCommitted`, is enveloped under the generation of the deltas this emitter
    /// sent of that segment, and under none when it sent none.
    pub(crate) struct StreamEmitter {
        run_id: RunId,
        producer: NodeId,
        port: String,
        stream_id: StreamId,
        sequence: i64,
        log: RunLog,
        /// The generation of each segment a delta this emitter sent named.
        segments: HashMap<SegmentId, GenerationId>,
    }

    impl StreamEmitter {
        pub(crate) fn new(run_id: RunId, producer: NodeId, port: String, log: RunLog) -> Self {
            Self {
                run_id,
                producer,
                port,
                stream_id: StreamId::generate(),
                sequence: 0,
                log,
                segments: HashMap::new(),
            }
        }

        /// The generation the envelope of `event` names: the one the event names, or the one of the
        /// deltas sent of the segment the event names.
        fn generation(&mut self, event: &StreamEvent) -> Option<GenerationId> {
            let segment_id = match event {
                StreamEvent::TextDelta(delta) => {
                    self.segments.insert(delta.segment_id, delta.generation_id);
                    return Some(delta.generation_id);
                }
                StreamEvent::TextSegmentCommitted(event) => event.segment_id,
                StreamEvent::SegmentQueued(event) => event.segment_id,
                StreamEvent::SynthesisCompleted(event) => event.segment_id,
                StreamEvent::AudioPlayed(event) => event.segment_id,
                StreamEvent::SegmentDiscarded(event) => event.segment_id,
                event => return event.generation_id(),
            };
            self.segments.get(&segment_id).copied()
        }

        fn envelope<Q>(&mut self, kind: String, payload: Q) -> EventEnvelope<Q> {
            self.sequence += 1;
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| {
                    i64::try_from(since.as_micros()).unwrap_or(i64::MAX)
                });
            EventEnvelope {
                schema_version: 1,
                event_id: EventId::generate(),
                run_id: self.run_id,
                session_id: None,
                stream_id: self.stream_id,
                sequence: self.sequence,
                producer: self.producer,
                port: self.port.clone(),
                kind,
                generation_id: None,
                timestamp,
                timebase: Timebase::WallTime,
                causation_id: None,
                payload,
            }
        }

        /// The envelope of `event`, recorded in the run's log.
        ///
        /// # Errors
        ///
        /// The log's [`EnvelopeRefusal`]; the event is then neither recorded nor to be sent.
        pub(crate) fn stream(
            &mut self,
            event: StreamEvent,
        ) -> Result<EventEnvelope<StreamEvent>, EnvelopeRefusal> {
            let generation_id = self.generation(&event);
            let mut envelope = self.envelope(event.name().to_owned(), event);
            envelope.generation_id = generation_id;
            self.log.record(envelope.clone())?;
            Ok(envelope)
        }

        /// The envelope of the control event `kind`, on the same stream.
        pub(crate) fn control<P: From<ControlKind>>(
            &mut self,
            kind: ControlKind,
        ) -> EventEnvelope<P> {
            self.envelope(kind.as_str().to_owned(), P::from(kind))
        }
    }
}

/// `envelope` carrying its event as `P`, built by `wrap`.
#[cfg(any(feature = "duplex", feature = "llm", feature = "stt", feature = "tts"))]
pub(crate) fn carry<P>(
    envelope: EventEnvelope<StreamEvent>,
    wrap: impl FnOnce(StreamEvent) -> P,
) -> EventEnvelope<P> {
    let EventEnvelope {
        schema_version,
        event_id,
        run_id,
        session_id,
        stream_id,
        sequence,
        producer,
        port,
        kind,
        generation_id,
        timestamp,
        timebase,
        causation_id,
        payload,
    } = envelope;
    EventEnvelope {
        schema_version,
        event_id,
        run_id,
        session_id,
        stream_id,
        sequence,
        producer,
        port,
        kind,
        generation_id,
        timestamp,
        timebase,
        causation_id,
        payload: wrap(payload),
    }
}
