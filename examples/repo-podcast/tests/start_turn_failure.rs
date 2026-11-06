//! A turn whose response cannot be asked for: the session the turn opened is closed, its closing
//! events read, and the time it ran handed on, rather than the session dropped while still open.

mod support;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use inference_audio_device::fake::FakeHost;
use inference_duplex::inference_audio::{AudioFrame, SampleClockPosition};
use inference_duplex::inference_core::model::Adapter;
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle, Unsupported,
};
use inference_testkit::duplex::mock::{MockSpeechModel, MockSpeechSession};
use repo_podcast::{Event, Show, default_hosts};
use support::{BRIEF, settings};

/// What became of one session when it was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ending {
    /// The ledger no longer holds the session open.
    closed: bool,
    /// The session's closing event was read from it.
    close_read: bool,
}

/// The duplex mock, every response it is asked for refused, as a transport that failed between
/// the open and the request would refuse it.
struct Refusing {
    template: MockSpeechModel,
    endings: Rc<RefCell<Vec<Ending>>>,
}

struct Refused {
    session: MockSpeechSession,
    close_read: bool,
    endings: Rc<RefCell<Vec<Ending>>>,
}

impl Drop for Refused {
    fn drop(&mut self) {
        self.endings.borrow_mut().push(Ending {
            closed: self.session.ledger().session().require_open().is_err(),
            close_read: self.close_read,
        });
    }
}

impl NativeSpeechModel for Refusing {
    type Session = Refused;

    fn adapter(&self) -> &Adapter {
        self.template.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        self.template.capabilities()
    }

    fn open(&mut self, config: SessionConfig) -> Result<Refused, OpenError> {
        let session = MockSpeechModel::native(Vec::new()).open(config)?;
        Ok(Refused {
            session,
            close_read: false,
            endings: Rc::clone(&self.endings),
        })
    }
}

impl SpeechSessionHandle for Refused {
    fn ledger(&self) -> &SessionLedger {
        self.session.ledger()
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        self.session.append_audio(frame)
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.session.commit_input()
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(Unsupported::ManualResponse))
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.session.cancel_response(response_id)
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.session.truncate_output(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        self.session.submit_tool_result(call_id, result)
    }

    fn close(&mut self) -> Result<(), SessionError> {
        self.session.close()
    }

    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent> {
        let event = self.session.next_event(wait);
        if matches!(event, Some(SessionEvent::SpeechSessionClosed(_))) {
            self.close_read = true;
        }
        event
    }
}

#[test]
fn a_refused_response_closes_the_session_the_turn_opened() {
    let endings = Rc::default();
    let model = Refusing {
        template: MockSpeechModel::native(Vec::new()),
        endings: Rc::clone(&endings),
    };
    assert!(
        model.capabilities().manual_response,
        "the show asks this model for each response"
    );
    let host = FakeHost::laptop();
    let mut show = Show::open(
        model,
        default_hosts(),
        BRIEF.to_owned(),
        &host,
        settings(Duration::from_millis(200)),
        |_| None,
    )
    .expect("the fake speaker opens");
    let mut events = Vec::new();
    for _ in 0..1_000 {
        events.extend(show.step());
        if show.is_over() {
            break;
        }
        let _ = host.pull(480);
    }
    assert!(show.is_over(), "the show ends: {events:#?}");
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::Problem { host: Some(0), detail } if detail.starts_with("no response")
        )),
        "{events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Billed { host: 0, .. })),
        "the session's time is handed on: {events:#?}"
    );
    drop(show);
    assert_eq!(
        *endings.borrow(),
        [Ending {
            closed: true,
            close_read: true,
        }],
        "one session opened, closed and its closing read"
    );
}
