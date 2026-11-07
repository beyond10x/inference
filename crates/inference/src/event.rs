//! `Event`: the SDK's event vocabulary, each variant carrying plain values and its raw session or
//! duplex event (`docs/design/sdk.md` §3.2), and [`Events`], the stream an agent's
//! [`events`](crate::Agent::events) returns.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use futures_core::Stream;
use inference_audio::AudioFrame;
use inference_core::usage::UsageRecorded;
use inference_duplex::SpeechCallId;
use inference_facade::events::SessionEvent;
use inference_tools::ToolCallId;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::Error;

/// What an agent's pump puts on its broadcast channel: an event, or its terminal error.
pub(crate) type Item = Result<Event, Arc<Error>>;

/// One event of an agent's session, in the SDK's vocabulary. Every stream ends after
/// [`Event::Closed`].
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum Event {
    /// The session is ready; `at` is the instant of the agent's [`Clock`](crate::Clock).
    Started {
        /// When the session became ready, on the agent's clock.
        at: Instant,
        /// The `SessionReady` it was made from.
        raw: SessionEvent,
    },
    /// The user was heard: a hypothesis (`final_` false) or a committed transcript.
    Heard {
        /// What was heard.
        text: String,
        /// Whether the transcript is committed.
        final_: bool,
        /// The transcript event it was made from.
        raw: SessionEvent,
    },
    /// A revision of a transcript already heard.
    Transcript {
        /// The revised text.
        text: String,
        /// The `TranscriptRevision` it was made from.
        raw: SessionEvent,
    },
    /// The agent said a segment of its reply.
    Said {
        /// The segment's text.
        text: String,
        /// The `TextSegmentCommitted` it was made from.
        raw: SessionEvent,
    },
    /// A frame of the agent's reply audio. It is yielded only while no sink is attached (no
    /// device run): then the application owns playback and cutting, a frame already yielded is
    /// not recalled by a cut (neither `agent.interrupt()` nor a barge-in), and
    /// [`Event::Interrupted`] tells it when to cut, on both topologies.
    Audio(AudioFrame),
    /// The model called a tool.
    ToolCalled {
        /// The tool's name.
        name: String,
        /// The call's arguments, when the agent's tool authority holds the call.
        arguments: serde_json::Value,
        /// The call's id.
        call_id: ToolCallId,
        /// The provider's own id of the call, for a native speech model.
        speech_call_id: Option<SpeechCallId>,
        /// The `ToolCallProposed` it was made from.
        raw: SessionEvent,
    },
    /// The usage of one model call.
    Usage(UsageRecorded),
    /// The reply in progress was cut, by `agent.interrupt()` or a barge-in.
    Interrupted {
        /// The `SessionInterrupted` it was made from; none for a cascade's own barge-in.
        raw: Option<SessionEvent>,
    },
    /// The session closed; the last event of every stream.
    Closed {
        /// The `SessionClosed` it was made from; none when the session's events ended without
        /// one.
        raw: Option<SessionEvent>,
    },
    /// A failure: `Error::Provider` for a failed response, `Error::Layer` for a failed run or
    /// model session.
    Error {
        /// The failure.
        error: Arc<Error>,
        /// The event it was made from, when one was.
        raw: Option<SessionEvent>,
    },
    /// The reader fell more than 1024 events behind and missed this many.
    Lagged(u64),
    /// A tool call that requires confirmation is held until `agent.confirm(call_id)` or
    /// `agent.reject(call_id)`.
    ConfirmationRequested {
        /// The call's id.
        call_id: ToolCallId,
        /// The tool's name.
        name: String,
        /// The call's arguments.
        arguments: serde_json::Value,
    },
}

impl Event {
    /// The session or duplex event this event was made from; none for [`Event::Lagged`],
    /// [`Event::ConfirmationRequested`] and an event made from none.
    #[must_use]
    pub fn raw(&self) -> Option<SessionEvent> {
        match self {
            Self::Started { raw, .. }
            | Self::Heard { raw, .. }
            | Self::Transcript { raw, .. }
            | Self::Said { raw, .. }
            | Self::ToolCalled { raw, .. } => Some(raw.clone()),
            Self::Audio(frame) => Some(SessionEvent::AudioChunk(frame.clone())),
            Self::Usage(record) => Some(SessionEvent::UsageRecorded(record.clone())),
            Self::Interrupted { raw } | Self::Closed { raw } | Self::Error { raw, .. } => {
                raw.clone()
            }
            Self::Lagged(_) | Self::ConfirmationRequested { .. } => None,
        }
    }
}

/// The event stream of an agent, from [`Agent::events`](crate::Agent::events).
pub struct Events {
    inner: Option<BroadcastStream<Item>>,
    closed: bool,
}

impl Events {
    pub(crate) fn over(receiver: Option<tokio::sync::broadcast::Receiver<Item>>) -> Self {
        Self {
            inner: receiver.map(BroadcastStream::new),
            closed: false,
        }
    }
}

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events")
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

impl Stream for Events {
    type Item = crate::Result<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.closed {
            return Poll::Ready(None);
        }
        let Some(inner) = self.inner.as_mut() else {
            return Poll::Ready(None);
        };
        match Pin::new(inner).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                self.closed = true;
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(BroadcastStreamRecvError::Lagged(n)))) => {
                Poll::Ready(Some(Ok(Event::Lagged(n))))
            }
            Poll::Ready(Some(Ok(Ok(event)))) => {
                if matches!(event, Event::Closed { .. }) {
                    self.closed = true;
                }
                Poll::Ready(Some(Ok(event)))
            }
            Poll::Ready(Some(Ok(Err(error)))) => {
                self.closed = true;
                Poll::Ready(Some(Err(Error::Layer {
                    message: error.to_string(),
                    source: None,
                })))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Event;

    /// Adding or removing a variant fails this match, which has no wildcard arm.
    #[test]
    fn every_variant_is_matched_by_name() {
        let event = Event::Lagged(1);
        let name = match event {
            Event::Started { .. } => "Started",
            Event::Heard { .. } => "Heard",
            Event::Transcript { .. } => "Transcript",
            Event::Said { .. } => "Said",
            Event::Audio(_) => "Audio",
            Event::ToolCalled { .. } => "ToolCalled",
            Event::Usage(_) => "Usage",
            Event::Interrupted { .. } => "Interrupted",
            Event::Closed { .. } => "Closed",
            Event::Error { .. } => "Error",
            Event::Lagged(_) => "Lagged",
            Event::ConfirmationRequested { .. } => "ConfirmationRequested",
        };
        assert_eq!(name, "Lagged");
        assert!(Event::Lagged(1).raw().is_none());
    }
}
