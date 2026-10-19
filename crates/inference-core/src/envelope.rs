//! The canonical event envelope of `ess/domains/stream.yaml`: `inference.stream.EventEnvelope`,
//! the clock its timestamp is read on, `inference.stream.Timebase`, and the identity of one
//! event, `inference.stream.EventId`.
//!
//! An [`EventEnvelope`] carries one event, its `payload`, with what a recorder, a router or a
//! replay needs to place it: the run, the optional session, the stream it is ordered on, its
//! sequence on that stream, the producing node and port, the event kind, the optional generation,
//! a timestamp on a named [`Timebase`] and the optional causing event. There is no single global
//! order across independently produced streams: `sequence` orders the events of one stream only,
//! and a recorder may assign a merge order of its own beside it. `kind` is the declared name of
//! the payload's event, such as `inference.stream.TranscriptHypothesis`, and is how a consumer
//! tells a control event (start, stop, commit, revise, cancel, error) from user content; no
//! separate control type is declared. The envelope enforces nothing itself: refusing a duplicate
//! event id, a non-increasing sequence or a generation that differs from the payload's is the
//! work of the log that records envelopes. Serialized names are the ESS names.

use serde::{Deserialize, Serialize};

use crate::id::{GenerationId, NodeId, RunId, SessionId, StreamId};

crate::uuid_newtype!(
    /// `inference.stream.EventId`: the stable identity of one routable event, for deduplication
    /// and tracing.
    EventId
);

/// `inference.stream.Timebase`: the clock an event timestamp is read on. Audio frame time,
/// inference execution time, wall time and playback time are distinct clocks, and an envelope
/// names its timebase so that events of different sources align without assuming their clocks
/// are identical. Serialized as written in ESS (`audio_frame_time`, `wall_time`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Timebase {
    /// Time on the audio sample clock of the stream.
    AudioFrameTime,
    /// Time as the inference computation counts it.
    InferenceExecutionTime,
    /// Time on the wall clock.
    WallTime,
    /// Time as the playback sink counts it.
    PlaybackTime,
}

impl Timebase {
    /// Every timebase, in the order `ess/domains/stream.yaml` lists them.
    pub const ALL: [Self; 4] = [
        Self::AudioFrameTime,
        Self::InferenceExecutionTime,
        Self::WallTime,
        Self::PlaybackTime,
    ];
}

/// `inference.stream.EventEnvelope`: one routable event, `payload`, with the fields that place it.
///
/// `session_id`, `generation_id` and `causation_id` are optional, written as `null` when absent
/// and read as absent when missing; every other field is required. `port` is opaque: whether it
/// names a port id or a port name is not settled. `timestamp` is a count on the clock `timebase`
/// names; its unit and origin are not settled either.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope<P> {
    pub schema_version: i64,
    pub event_id: EventId,
    pub run_id: RunId,
    pub session_id: Option<SessionId>,
    pub stream_id: StreamId,
    pub sequence: i64,
    pub producer: NodeId,
    pub port: String,
    pub kind: String,
    pub generation_id: Option<GenerationId>,
    pub timestamp: i64,
    pub timebase: Timebase,
    pub causation_id: Option<EventId>,
    pub payload: P,
}
