//! Procedures any adapter runs to give evidence for a capability it claims.
//!
//! [`native_duplex_evidence`] decides whether a model may claim `native_duplex`: with one
//! response in progress, input sent after its output began is accepted, and output whose content
//! derives from that input is emitted on the same response before it completes, with no second
//! response id. A model that answers new input by cancelling and restarting gives a second
//! response id and fails; it may claim `concurrent_input_output` at most. The specification's
//! own scenario of this shape runs against a store and holds by construction; this procedure is
//! what runs against an adapter.
//!
//! Dependency is shown by content, never by timing alone: the input sent during output says a
//! `marker` the output before it did not contain, and the output after it must contain it. The
//! output before it is everything recorded before the input is sent, not only the event that
//! showed output began; and an opening whose transcript already says the marker is refused as
//! [`NoNativeDuplex::MarkerInOpening`], since output saying it then depends on nothing later. A
//! live adapter runs it with recorded speech that says the marker; a mock runs it with whatever
//! its recogniser hears.

use std::error::Error;
use std::fmt;
use std::time::Duration;

use inference_audio::AudioFrame;

use crate::{
    NativeSpeechModel, OpenError, ResponseId, ResponseState, SessionConfig, SessionError,
    SessionEvent, SessionFailure, SpeechSessionHandle, TurnDetection,
};

/// What [`native_duplex_evidence`] sends and looks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDuplexProbe {
    /// The session configuration; `auto_interrupt` should be off, so that the input sent during
    /// output is not barge-in.
    pub config: SessionConfig,
    /// The user turn that starts the response: speech, then enough silence to end it. With turn
    /// detection off the procedure commits it and, with `auto_response` off, creates the response.
    pub opening: Vec<AudioFrame>,
    /// The input sent once the response's output began. With turn detection off the procedure
    /// commits it.
    pub during: Vec<AudioFrame>,
    /// A word the `during` input says, which output that depends on it repeats; compared without
    /// regard to case.
    pub marker: String,
    /// How long one wait for the next event lasts.
    pub wait: Duration,
    /// How many waits in a row may deliver nothing before the procedure gives up.
    pub patience: usize,
}

/// The evidence: the response, and its transcript before and after the input sent during it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeDuplexEvidence {
    pub response_id: ResponseId,
    pub before: String,
    pub after: String,
}

/// Why a model gave no evidence for `native_duplex`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoNativeDuplex {
    /// The session did not open.
    Open(OpenError),
    /// An operation of the procedure was refused before the input during output.
    Refused(SessionError),
    /// No response began output within the patience.
    NoOutput,
    /// The input sent during output was refused.
    InputRefused(SessionError),
    /// A second response was created before the first one completed, whether or not the first
    /// was cancelled before it: a restart.
    Restarted {
        first: ResponseId,
        second: ResponseId,
    },
    /// The response ended other than completed.
    Ended {
        response_id: ResponseId,
        state: ResponseState,
    },
    /// The response completed without output that says the marker, or said it before the input.
    /// `before` holds every output transcript recorded before the input was sent.
    NotDependent { before: String, after: String },
    /// The opening's transcript already says the marker, so output saying it shows no
    /// dependency on the input sent during output: the probe is unfit, nothing was sent of it.
    MarkerInOpening { heard: String },
    /// The session failed.
    Failed(SessionFailure),
    /// The response did not complete within the patience.
    Silent,
}

impl fmt::Display for NoNativeDuplex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => write!(f, "the session did not open: {error}"),
            Self::Refused(error) => write!(f, "the procedure was refused: {error}"),
            Self::NoOutput => f.write_str("no response began output"),
            Self::InputRefused(error) => write!(f, "input during output was refused: {error}"),
            Self::Restarted { first, second } => write!(
                f,
                "response {second} was created while {first} was in progress: a restart"
            ),
            Self::Ended { response_id, state } => {
                write!(f, "response {response_id} ended {state:?}")
            }
            Self::NotDependent { before, after } => write!(
                f,
                "no output derived from the input: before {before:?}, after {after:?}"
            ),
            Self::MarkerInOpening { heard } => write!(
                f,
                "the opening already says the marker, so the probe shows nothing: {heard:?}"
            ),
            Self::Failed(failure) => write!(f, "the session failed: {failure}"),
            Self::Silent => f.write_str("the response did not complete"),
        }
    }
}

impl Error for NoNativeDuplex {}

/// Runs the `native_duplex` evidence procedure against `model`, then closes the session.
///
/// # Errors
///
/// [`NoNativeDuplex`] naming the first step the model failed.
pub fn native_duplex_evidence<M: NativeSpeechModel>(
    model: &mut M,
    probe: &NativeDuplexProbe,
) -> Result<NativeDuplexEvidence, NoNativeDuplex> {
    let mut session = model
        .open(probe.config.clone())
        .map_err(NoNativeDuplex::Open)?;
    let result = run(&mut session, probe);
    let _ = session.close();
    result
}

fn run<S: SpeechSessionHandle>(
    session: &mut S,
    probe: &NativeDuplexProbe,
) -> Result<NativeDuplexEvidence, NoNativeDuplex> {
    let manual_turns = probe.config.turn_detection == TurnDetection::Disabled;
    for frame in &probe.opening {
        session
            .append_audio(frame.clone())
            .map_err(NoNativeDuplex::Refused)?;
    }
    if manual_turns {
        session.commit_input().map_err(NoNativeDuplex::Refused)?;
    }
    if !probe.config.auto_response {
        session.create_response().map_err(NoNativeDuplex::Refused)?;
    }

    let marker = probe.marker.to_lowercase();
    let mut response = None;
    let mut before = String::new();
    let mut idle = 0;
    loop {
        let Some(event) = session.next_event(probe.wait) else {
            idle += 1;
            if idle > probe.patience {
                return Err(NoNativeDuplex::NoOutput);
            }
            continue;
        };
        idle = 0;
        let output = matches!(
            &event,
            SessionEvent::OutputTranscriptDelta(delta) if Some(delta.response_id) == response
        ) || matches!(
            &event,
            SessionEvent::OutputAudio(audio) if Some(audio.response_id) == response
        );
        opening(event, &marker, &mut response, &mut before)?;
        if output {
            break;
        }
    }
    let Some(response_id) = response else {
        return Err(NoNativeDuplex::NoOutput);
    };
    for _ in 0..session.ledger().undelivered() {
        let Some(event) = session.next_event(Duration::ZERO) else {
            break;
        };
        opening(event, &marker, &mut response, &mut before)?;
    }

    for frame in &probe.during {
        session
            .append_audio(frame.clone())
            .map_err(NoNativeDuplex::InputRefused)?;
    }
    if manual_turns {
        session
            .commit_input()
            .map_err(NoNativeDuplex::InputRefused)?;
    }

    let mut after = String::new();
    let mut cancelled = false;
    let mut idle = 0;
    loop {
        let Some(event) = session.next_event(probe.wait) else {
            idle += 1;
            if idle > probe.patience {
                return Err(if cancelled {
                    NoNativeDuplex::Ended {
                        response_id,
                        state: ResponseState::Cancelled,
                    }
                } else {
                    NoNativeDuplex::Silent
                });
            }
            continue;
        };
        idle = 0;
        match event {
            SessionEvent::ResponseCreated(created) => {
                return Err(NoNativeDuplex::Restarted {
                    first: response_id,
                    second: created.response_id,
                });
            }
            SessionEvent::OutputTranscriptDelta(delta) if delta.response_id == response_id => {
                after.push_str(&delta.text);
            }
            SessionEvent::ResponseCompleted(done) if done.response_id == response_id => break,
            SessionEvent::ResponseCancelled(gone) if gone.response_id == response_id => {
                cancelled = true;
            }
            event => ended(&event, Some(response_id))?,
        }
    }

    if marker.is_empty()
        || before.to_lowercase().contains(&marker)
        || !after.to_lowercase().contains(&marker)
    {
        return Err(NoNativeDuplex::NotDependent { before, after });
    }
    Ok(NativeDuplexEvidence {
        response_id,
        before,
        after,
    })
}

/// One event before the input during output was sent: the response it creates, the output it
/// adds to `before`, an opening transcript that already says `marker`, or the end it stands for.
fn opening(
    event: SessionEvent,
    marker: &str,
    response: &mut Option<ResponseId>,
    before: &mut String,
) -> Result<(), NoNativeDuplex> {
    match event {
        SessionEvent::ResponseCreated(created) => match *response {
            None => *response = Some(created.response_id),
            Some(first) => {
                return Err(NoNativeDuplex::Restarted {
                    first,
                    second: created.response_id,
                });
            }
        },
        SessionEvent::InputTranscribed(heard)
            if !marker.is_empty() && heard.text.to_lowercase().contains(marker) =>
        {
            return Err(NoNativeDuplex::MarkerInOpening { heard: heard.text });
        }
        SessionEvent::OutputTranscriptDelta(delta) if Some(delta.response_id) == *response => {
            before.push_str(&delta.text);
        }
        event => ended(&event, *response)?,
    }
    Ok(())
}

/// The failure an event that ends the response other than by completion, or fails the session,
/// stands for; any other event is passed over.
fn ended(event: &SessionEvent, response: Option<ResponseId>) -> Result<(), NoNativeDuplex> {
    match event {
        SessionEvent::ResponseCancelled(cancelled) if Some(cancelled.response_id) == response => {
            Err(NoNativeDuplex::Ended {
                response_id: cancelled.response_id,
                state: ResponseState::Cancelled,
            })
        }
        SessionEvent::ResponseFailed(failed) if Some(failed.response_id) == response => {
            Err(NoNativeDuplex::Ended {
                response_id: failed.response_id,
                state: ResponseState::Failed,
            })
        }
        SessionEvent::ResponseCompleted(done) if Some(done.response_id) == response => {
            Err(NoNativeDuplex::Ended {
                response_id: done.response_id,
                state: ResponseState::Completed,
            })
        }
        SessionEvent::SpeechSessionFailed(failed) => {
            Err(NoNativeDuplex::Failed(failed.failure.clone()))
        }
        _ => Ok(()),
    }
}
