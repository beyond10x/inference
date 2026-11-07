//! `inference::Payload`: one enum carrying every port modality the workspace's nodes speak and
//! implementing every payload trait of the workspace (`docs/design/sdk.md` §3.4).
//!
//! An application that declares a graph over the workspace's nodes names [`Payload`] as the
//! payload of its run and of its session, and declares no enum of its own. Its variants are the
//! modalities the nodes speak: audio frames, `inference.stream` events, the usage records a model
//! node sends on its `usage` port, the steps of a tool call, the events a native speech model's
//! session reports, a frame of a segment as a synthesis emitted it and the receipt of its
//! playback, and control events.
//!
//! A frame of a segment and the receipt of its playback are not `Clone`, and the payload is,
//! since a fan-out copies what it hands on: each travels in a [`Handoff`], which its first reader
//! takes and every copy then holds empty, so the frames a receipt is made of exist once.
//!
//! The payload implements `AudioPayload`, `TurnPayload`, `GatePayload`, `ToolPayload`,
//! `SttPayload`, `ModelPayload`, `TtsPayload`, `SpeechPayload` and `DuplexPayload` of
//! `inference-pipeline`, `SessionPayload` of `inference-facade` and, with the feature
//! `audio-host`, `PlaybackPayload` and `DevicePayload` of `inference-audio-device`. Its session
//! events are the facade's: a frame is `AudioChunk`, an `inference.stream` event the variant the
//! session vocabulary has for it, a usage record `UsageRecorded`, a tool call's step its
//! `ToolCall…` event and a native speech model's event its `Speech…` event; a control event, a
//! speech, turn, queued or discarded segment event, a segment's frame and a receipt have none.
//!
//! `Payload` is `#[non_exhaustive]`: the variant [`Payload::Receipt`] exists only with the
//! feature `audio-host`, and a match outside this crate ends in a wildcard arm whatever the
//! features.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use inference_audio::AudioFrame;
#[cfg(feature = "audio-host")]
use inference_audio_device::acknowledging::{Playable, PlaybackPayload};
#[cfg(feature = "audio-host")]
use inference_audio_device::node::DevicePayload;
use inference_core::envelope::ControlKind;
use inference_core::usage::UsageRecorded;
use inference_duplex::{ResponseFailed, SpeechSessionClosed, SpeechSessionFailed, SpeechStarted};
use inference_facade::events::{SessionEvent, SessionPayload};
use inference_pipeline::nodes::commit_gate::{GatePayload, Gated};
use inference_pipeline::nodes::duplex::DuplexPayload;
use inference_pipeline::nodes::llm::{Carried, ModelPayload};
use inference_pipeline::nodes::log::StreamEvent;
use inference_pipeline::nodes::speech::{Said, SpeechPayload};
use inference_pipeline::nodes::stt::{Heard, SttPayload};
use inference_pipeline::nodes::tts::{Spoken, TtsPayload};
use inference_pipeline::nodes::turn_policy::{TurnEvent, TurnPayload};
use inference_pipeline::nodes::vad::{AudioPayload, SpeechActivity};
use inference_pipeline::nodes::{ToolCallEvent, ToolCallUpdate, ToolPayload};
#[cfg(feature = "audio-host")]
use inference_text::SegmentId;
#[cfg(feature = "audio-host")]
use inference_tts::PlaybackReceipt;
use inference_tts::SegmentAudio;

/// A value handed along one edge and taken once: the first reader takes it, and every copy of
/// the handoff, a fan-out's among them, then holds nothing.
pub struct Handoff<T>(Arc<Mutex<Option<T>>>);

impl<T> Handoff<T> {
    /// A handoff holding `value`.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }

    /// The value, if no reader took it before.
    #[must_use]
    pub fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    /// Whether a reader took the value.
    #[must_use]
    pub fn taken(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
    }
}

impl<T> Clone for Handoff<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> fmt::Debug for Handoff<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handoff")
            .field("taken", &self.taken())
            .finish()
    }
}

/// What the edges, exposures and sessions of a run over the workspace's nodes carry; see the
/// module documentation.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Payload {
    /// A frame of audio; its envelope's `kind` is `inference.audio.AudioChunk`.
    Audio(AudioFrame),
    /// An `inference.stream` event; its envelope's `kind` is the event's declared name.
    Stream(StreamEvent),
    /// The usage of one model call, sent on a model node's `usage` port; its envelope's `kind`
    /// is [`UsageRecorded::KIND`].
    Usage(UsageRecorded),
    /// A step of a tool call along the tool path.
    Tool(ToolCallUpdate),
    /// `inference.duplex.SpeechStarted`: a native speech model heard the user start speaking.
    SpeechStarted(SpeechStarted),
    /// `inference.duplex.ResponseFailed`: a native speech model's response failed.
    ResponseFailed(ResponseFailed),
    /// `inference.duplex.SpeechSessionClosed`: a native speech model closed its session.
    SpeechClosed(SpeechSessionClosed),
    /// `inference.duplex.SpeechSessionFailed`: a native speech model's session failed.
    SpeechFailed(SpeechSessionFailed),
    /// A frame of a segment as the synthesis emitted it, taken once by the sink.
    Speech(Handoff<SegmentAudio>),
    /// The sink's receipt of the playback of a segment, taken once by the TTS node.
    #[cfg(feature = "audio-host")]
    Receipt(Handoff<(SegmentId, PlaybackReceipt)>),
    /// A control event; its envelope's `kind` is [`ControlKind::as_str`].
    Control(ControlKind),
}

impl From<ControlKind> for Payload {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl AudioPayload for Payload {
    fn into_audio(self) -> Result<AudioFrame, Self> {
        match self {
            Self::Audio(frame) => Ok(frame),
            other => Err(other),
        }
    }

    fn from_activity(activity: SpeechActivity) -> Self {
        Self::Stream(match activity {
            SpeechActivity::Started(started) => StreamEvent::SpeechStarted(started),
            SpeechActivity::Ended(ended) => StreamEvent::SpeechEnded(ended),
        })
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl TurnPayload for Payload {
    fn into_activity(self) -> Result<SpeechActivity, Self> {
        match self {
            Self::Stream(StreamEvent::SpeechStarted(started)) => {
                Ok(SpeechActivity::Started(started))
            }
            Self::Stream(StreamEvent::SpeechEnded(ended)) => Ok(SpeechActivity::Ended(ended)),
            other => Err(other),
        }
    }

    fn from_turn(event: TurnEvent) -> Self {
        Self::Stream(match event {
            TurnEvent::Opened(opened) => StreamEvent::TurnOpened(opened),
            TurnEvent::Completed(completed) => StreamEvent::TurnCompleted(completed),
        })
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl GatePayload for Payload {
    fn gated(&self) -> Gated {
        let Self::Stream(event) = self else {
            return Gated::Other;
        };
        match event {
            StreamEvent::TranscriptHypothesis(event) => Gated::Hypothesis {
                span_id: event.span_id,
            },
            StreamEvent::TranscriptRevision(event) => Gated::Revision {
                span_id: event.span_id,
            },
            StreamEvent::TranscriptCommitted(event) => Gated::SpanCommitted {
                span_id: event.span_id,
            },
            StreamEvent::TurnOpened(opened) => Gated::Turn(TurnEvent::Opened(*opened)),
            StreamEvent::TurnCompleted(completed) => Gated::Turn(TurnEvent::Completed(*completed)),
            StreamEvent::TextDelta(delta) => Gated::Delta {
                segment_id: delta.segment_id,
                generation_id: delta.generation_id,
            },
            StreamEvent::TextSegmentCommitted(committed) => Gated::SegmentCommitted {
                segment_id: committed.segment_id,
            },
            StreamEvent::SegmentDiscarded(discarded) => Gated::SegmentDiscarded {
                segment_id: discarded.segment_id,
            },
            StreamEvent::GenerationCompleted(ended) => Gated::GenerationEnded {
                generation_id: ended.generation_id,
            },
            StreamEvent::GenerationCancelled(ended) => Gated::GenerationEnded {
                generation_id: ended.generation_id,
            },
            StreamEvent::GenerationSuperseded(ended) => Gated::GenerationEnded {
                generation_id: ended.generation_id,
            },
            _ => Gated::Other,
        }
    }
}

impl ToolPayload for Payload {
    fn into_tool_call(self) -> Result<ToolCallUpdate, Self> {
        match self {
            Self::Tool(update) => Ok(update),
            other => Err(other),
        }
    }

    fn from_tool_call(update: ToolCallUpdate) -> Self {
        Self::Tool(update)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl SttPayload for Payload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn usage(recorded: UsageRecorded) -> Option<Self> {
        Some(Self::Usage(recorded))
    }

    fn heard(self) -> Heard {
        match self {
            Self::Audio(frame) => Heard::Audio(frame),
            Self::Stream(event) => Heard::Stream(event),
            Self::Control(kind) => Heard::Control(kind),
            _ => Heard::Other,
        }
    }
}

impl ModelPayload for Payload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn usage(recorded: UsageRecorded) -> Option<Self> {
        Some(Self::Usage(recorded))
    }

    fn carried(self) -> Carried {
        match self {
            Self::Stream(event) => Carried::Stream(event),
            Self::Tool(update) => Carried::Tool(update),
            Self::Usage(recorded) => Carried::Usage(recorded),
            Self::Control(kind) => Carried::Control(kind),
            _ => Carried::Control(ControlKind::Error),
        }
    }
}

impl TtsPayload for Payload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn audio(frame: AudioFrame) -> Self {
        Self::Audio(frame)
    }

    fn usage(recorded: UsageRecorded) -> Option<Self> {
        Some(Self::Usage(recorded))
    }

    fn spoken(self) -> Spoken {
        match self {
            Self::Stream(event) => Spoken::Stream(event),
            Self::Control(kind) => Spoken::Control(kind),
            _ => Spoken::Other,
        }
    }
}

impl SpeechPayload for Payload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn speech(audio: SegmentAudio) -> Self {
        Self::Speech(Handoff::new(audio))
    }

    fn said(self) -> Said {
        match self {
            Self::Stream(event) => Said::Stream(event),
            #[cfg(feature = "audio-host")]
            Self::Receipt(receipt) => match receipt.take() {
                Some((segment_id, receipt)) => Said::Receipt {
                    segment_id,
                    receipt,
                },
                None => Said::Other,
            },
            Self::Control(kind) => Said::Control(kind),
            _ => Said::Other,
        }
    }
}

impl DuplexPayload for Payload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn audio(frame: AudioFrame) -> Self {
        Self::Audio(frame)
    }

    fn usage(recorded: UsageRecorded) -> Self {
        Self::Usage(recorded)
    }

    fn speech_started(event: SpeechStarted) -> Self {
        Self::SpeechStarted(event)
    }

    fn response_failed(event: ResponseFailed) -> Self {
        Self::ResponseFailed(event)
    }

    fn closed(event: SpeechSessionClosed) -> Self {
        Self::SpeechClosed(event)
    }

    fn failed(event: SpeechSessionFailed) -> Self {
        Self::SpeechFailed(event)
    }

    fn into_audio(self) -> Result<AudioFrame, Self> {
        match self {
            Self::Audio(frame) => Ok(frame),
            other => Err(other),
        }
    }
}

/// A session writes its control events as [`Payload::Control`] and delivers what the run's
/// egresses carry as the module documentation lists.
impl SessionPayload for Payload {
    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }

    fn session_event(&self) -> Option<SessionEvent> {
        Some(match self {
            Self::Audio(frame) => SessionEvent::AudioChunk(frame.clone()),
            Self::Stream(event) => return stream_session_event(event),
            Self::Usage(recorded) => SessionEvent::UsageRecorded(recorded.clone()),
            Self::Tool(update) => match update.event.clone() {
                ToolCallEvent::Proposed(event) => SessionEvent::ToolCallProposed(event),
                ToolCallEvent::Authorized(event) => SessionEvent::ToolCallAuthorized(event),
                ToolCallEvent::Rejected(event) => SessionEvent::ToolCallRejected(event),
                ToolCallEvent::Started(event) => SessionEvent::ToolCallStarted(event),
                ToolCallEvent::Result(event) => SessionEvent::ToolCallResult(event),
                ToolCallEvent::Failed(event) => SessionEvent::ToolCallFailed(event),
            },
            Self::SpeechStarted(event) => SessionEvent::SpeechStarted(*event),
            Self::ResponseFailed(event) => SessionEvent::SpeechResponseFailed(event.clone()),
            Self::SpeechClosed(event) => SessionEvent::SpeechSessionClosed(*event),
            Self::SpeechFailed(event) => SessionEvent::SpeechSessionFailed(event.clone()),
            Self::Speech(_) | Self::Control(_) => return None,
            #[cfg(feature = "audio-host")]
            Self::Receipt(_) => return None,
        })
    }
}

/// The session event an `inference.stream` event is delivered as: every event of the session
/// vocabulary; none for a speech, turn, queued or discarded segment event, which the vocabulary
/// does not carry.
fn stream_session_event(event: &StreamEvent) -> Option<SessionEvent> {
    Some(match event.clone() {
        StreamEvent::TranscriptHypothesis(event) => SessionEvent::TranscriptHypothesis(event),
        StreamEvent::TranscriptRevision(event) => SessionEvent::TranscriptRevision(event),
        StreamEvent::TranscriptCommitted(event) => SessionEvent::TranscriptCommitted(event),
        StreamEvent::GenerationStarted(event) => SessionEvent::GenerationStarted(event),
        StreamEvent::GenerationCompleted(event) => SessionEvent::GenerationCompleted(event),
        StreamEvent::GenerationSuperseded(event) => SessionEvent::GenerationSuperseded(event),
        StreamEvent::GenerationCancelled(event) => SessionEvent::GenerationCancelled(event),
        StreamEvent::GenerationRefused(event) => SessionEvent::GenerationRefused(event),
        StreamEvent::TextDelta(event) => SessionEvent::TextDelta(event),
        StreamEvent::TextSegmentCommitted(event) => SessionEvent::TextSegmentCommitted(event),
        StreamEvent::SynthesisCompleted(event) => SessionEvent::SynthesisCompleted(event),
        StreamEvent::AudioPlayed(event) => SessionEvent::AudioPlayed(event),
        StreamEvent::SpeechStarted(_)
        | StreamEvent::SpeechEnded(_)
        | StreamEvent::TurnOpened(_)
        | StreamEvent::TurnCompleted(_)
        | StreamEvent::SegmentQueued(_)
        | StreamEvent::SegmentDiscarded(_) => return None,
    })
}

#[cfg(feature = "audio-host")]
impl PlaybackPayload for Payload {
    fn playable(self) -> Playable {
        match self {
            Self::Speech(audio) => audio.take().map_or(Playable::Other, Playable::Audio),
            Self::Stream(StreamEvent::SynthesisCompleted(completed)) => {
                Playable::Completed(completed.segment_id)
            }
            Self::Stream(StreamEvent::TurnOpened(opened)) => Playable::TurnOpened(opened.turn_id),
            Self::Stream(StreamEvent::GenerationSuperseded(superseded)) => {
                Playable::Superseded(superseded.generation_id)
            }
            Self::Stream(StreamEvent::GenerationStarted(started)) => Playable::Started {
                generation_id: started.generation_id,
                epoch: started.epoch,
            },
            _ => Playable::Other,
        }
    }

    fn receipt(segment_id: SegmentId, receipt: PlaybackReceipt) -> Self {
        Self::Receipt(Handoff::new((segment_id, receipt)))
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

#[cfg(feature = "audio-host")]
impl DevicePayload for Payload {
    fn from_audio(frame: AudioFrame) -> Self {
        Self::Audio(frame)
    }

    fn into_audio(self) -> Result<AudioFrame, Self> {
        match self {
            Self::Audio(frame) => Ok(frame),
            other => Err(other),
        }
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

#[cfg(test)]
mod tests {
    use inference_audio::SampleClockPosition;
    use inference_core::id::{ResponseId, SpeechSessionId};
    use inference_core::model::{AdapterId, ModelId};
    use inference_core::usage::{UsageRecordId, UsageRecorded};
    use inference_duplex::{
        ResponseFailed, SessionFailure, SessionFailureKind, SpeechSessionClosed,
        SpeechSessionFailed, SpeechStarted,
    };
    use inference_facade::events::{SessionEvent, SessionPayload};
    use inference_pipeline::nodes::duplex::DuplexPayload;
    use inference_pipeline::nodes::llm::ModelPayload;
    use inference_pipeline::nodes::stt::SttPayload;
    use inference_pipeline::nodes::tts::TtsPayload;

    use super::{Handoff, Payload};

    fn failure() -> SessionFailure {
        SessionFailure {
            kind: SessionFailureKind::ProviderError,
            detail: "the provider failed".to_owned(),
        }
    }

    fn recorded() -> UsageRecorded {
        UsageRecorded {
            usage_record_id: UsageRecordId::generate(),
            run_id: None,
            node_id: None,
            session_id: None,
            speech_session_id: None,
            response_id: None,
            adapter_id: AdapterId::generate(),
            model_id: ModelId::generate(),
            upstream_model: None,
            started_at_ms: 1,
            ended_at_ms: 2,
            r#final: true,
            quantities: Vec::new(),
        }
    }

    #[test]
    fn a_closed_speech_session_is_delivered_as_its_session_event() {
        let session_id = SpeechSessionId::generate();
        let Some(SessionEvent::SpeechSessionClosed(closed)) =
            Payload::closed(SpeechSessionClosed { session_id }).session_event()
        else {
            panic!("a closed speech session is SpeechSessionClosed");
        };
        assert_eq!(closed.session_id, session_id);
    }

    #[test]
    fn a_speech_start_is_delivered_as_its_session_event() {
        let position = SampleClockPosition::new(4_800).expect("in range");
        let Some(SessionEvent::SpeechStarted(started)) = Payload::speech_started(SpeechStarted {
            session_id: SpeechSessionId::generate(),
            position,
        })
        .session_event() else {
            panic!("a speech start is SpeechStarted");
        };
        assert_eq!(started.position, position);
    }

    #[test]
    fn a_failed_response_is_delivered_as_its_session_event() {
        let response_id = ResponseId::generate();
        let Some(SessionEvent::SpeechResponseFailed(failed)) =
            Payload::response_failed(ResponseFailed {
                response_id,
                failure: failure(),
            })
            .session_event()
        else {
            panic!("a failed response is SpeechResponseFailed");
        };
        assert_eq!(failed.response_id, response_id);
    }

    #[test]
    fn a_failed_speech_session_is_delivered_as_its_session_event() {
        let session_id = SpeechSessionId::generate();
        let Some(SessionEvent::SpeechSessionFailed(failed)) =
            Payload::failed(SpeechSessionFailed {
                session_id,
                failure: failure(),
            })
            .session_event()
        else {
            panic!("a failed speech session is SpeechSessionFailed");
        };
        assert_eq!(failed.session_id, session_id);
    }

    #[test]
    fn a_model_usage_record_is_carried_and_delivered_as_usage_recorded() {
        let record = recorded();
        let Some(Payload::Usage(carried)) = <Payload as ModelPayload>::usage(record.clone()) else {
            panic!("the payload carries a model node's usage");
        };
        assert_eq!(carried, record);
        assert_eq!(
            Payload::Usage(carried).session_event(),
            Some(SessionEvent::UsageRecorded(record))
        );
    }

    #[test]
    fn every_model_node_finds_its_usage_carried() {
        let record = recorded();
        for carried in [
            <Payload as SttPayload>::usage(record.clone()),
            <Payload as TtsPayload>::usage(record.clone()),
            Some(<Payload as DuplexPayload>::usage(record.clone())),
        ] {
            assert!(
                matches!(carried, Some(Payload::Usage(ref usage)) if *usage == record),
                "{carried:?}"
            );
        }
    }

    #[test]
    fn a_handoff_and_every_copy_of_it_give_the_value_once() {
        let handoff = Handoff::new(String::from("frame"));
        let copy = handoff.clone();
        assert!(!copy.taken());
        assert_eq!(copy.take().as_deref(), Some("frame"));
        assert!(handoff.taken());
        assert_eq!(handoff.take(), None, "a copy holds nothing once taken");
        assert_eq!(copy.take(), None);
    }
}
