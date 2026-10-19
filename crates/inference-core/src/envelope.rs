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
//! separate control type is declared, and [`ControlKind`] names the control kinds only. The
//! envelope enforces nothing itself: refusing a duplicate
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

/// The kinds of control event: start, stop, commit, revise, cancel and error, which must not be
/// mistaken for user content. A control event is told from user content by the `kind` of its
/// [`EventEnvelope`] alone, which is [`ControlKind::as_str`], the design's lowercase word; no
/// separate control type is declared, so these are names only and carry no payload of their
/// own. The end of a producer's input is a `stop`.
///
/// read from: `docs/design/inference.md:297`
/// read from: `ess/domains/stream.yaml:63-67`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlKind {
    /// `start`.
    Start,
    /// `stop`; the end of a producer's input among others.
    Stop,
    /// `commit`.
    Commit,
    /// `revise`.
    Revise,
    /// `cancel`.
    Cancel,
    /// `error`.
    Error,
}

impl ControlKind {
    /// Every control kind, in the order the design lists them.
    pub const ALL: [Self; 6] = [
        Self::Start,
        Self::Stop,
        Self::Commit,
        Self::Revise,
        Self::Cancel,
        Self::Error,
    ];

    /// The `kind` an envelope of this control event carries: the design's lowercase word,
    /// `stop` for [`ControlKind::Stop`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Commit => "commit",
            Self::Revise => "revise",
            Self::Cancel => "cancel",
            Self::Error => "error",
        }
    }

    /// The control kind `kind` names, if it names one.
    #[must_use]
    pub fn of(kind: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|control| control.as_str() == kind)
    }
}

/// Whether an envelope of `kind` carries a control event rather than user content.
#[must_use]
pub fn is_control_kind(kind: &str) -> bool {
    ControlKind::of(kind).is_some()
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

#[cfg(test)]
mod tests {
    use super::{ControlKind, is_control_kind};

    #[test]
    fn the_six_control_kinds_are_the_designs_lowercase_words_and_nothing_else_is_one() {
        let words: Vec<&str> = ControlKind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(
            words,
            ["start", "stop", "commit", "revise", "cancel", "error"]
        );
        for kind in ControlKind::ALL {
            assert_eq!(ControlKind::of(kind.as_str()), Some(kind));
            assert!(is_control_kind(kind.as_str()));
        }
        for content in ["inference.stream.TextDelta", "Stop", "end", ""] {
            assert!(
                !is_control_kind(content),
                "{content:?} is not a control kind"
            );
        }
    }
}
