//! Adversary pass 2: barge-in with the events the Nova Sonic binding delivers.
//!
//! `wiring.rs` runs the Nova session against the testkit's `server_turns` mock. The Nova Sonic
//! binding records no end of speech, and records speech started only at the provider's
//! interruption notice (also over a response it already completed) or the turn's first
//! transcript (`adapters/inference-aws/src/duplex.rs`, `report_speech`). Here the same mock runs
//! behind a session that delivers what the binding delivers: never `SpeechStopped`.

use std::time::Duration;

use inference_audio_device::fake::FakeHost;
use inference_duplex::inference_audio::{AudioFrame, SampleClockPosition};
use inference_duplex::inference_core::model::Adapter;
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle,
};
use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn};
use realtime_voice::{Conversation, Ending, Note, Options, nova_config};
use serde_json::json;

type MockSession = <MockSpeechModel as NativeSpeechModel>::Session;

/// The mock, its session delivering no `SpeechStopped`, as the Nova Sonic binding records none.
struct NovaEvents(MockSpeechModel);

struct NovaEventsSession(MockSession);

impl NativeSpeechModel for NovaEvents {
    type Session = NovaEventsSession;

    fn adapter(&self) -> &Adapter {
        self.0.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        self.0.capabilities()
    }

    fn open(&mut self, config: SessionConfig) -> Result<NovaEventsSession, OpenError> {
        self.0.open(config).map(NovaEventsSession)
    }
}

impl SpeechSessionHandle for NovaEventsSession {
    fn ledger(&self) -> &SessionLedger {
        self.0.ledger()
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        self.0.append_audio(frame)
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.0.commit_input()
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.0.create_response()
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.0.cancel_response(response_id)
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.0.truncate_output(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        self.0.submit_tool_result(call_id, result)
    }

    fn close(&mut self) -> Result<(), SessionError> {
        self.0.close()
    }

    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent> {
        loop {
            match self.0.next_event(wait)? {
                SessionEvent::SpeechStopped(_) => {}
                event => return Some(event),
            }
        }
    }
}

const TEN_MS: usize = 480 * 2;
const TEN_MS_FRAMES: usize = 480;

fn capture(host: &FakeHost, millis: usize, sample: f32) {
    for _ in 0..millis / 10 {
        assert!(host.feed(&[sample; TEN_MS]), "the microphone runs");
    }
}

fn say(heard: &str, reply: &str) -> ScriptedTurn {
    ScriptedTurn {
        heard: heard.to_owned(),
        reply: Reply::Say(reply.to_owned()),
    }
}

/// The operator interrupts an answer the provider finished generating and the speaker is still
/// playing, the common case when generation outruns playback (the speaker queues four seconds).
/// The acceptance: "interrupts it mid-answer and hears it stop".
#[test]
fn speech_over_a_nova_answer_still_playing_stops_playout() {
    let host = FakeHost::laptop();
    let mut model = NovaEvents(MockSpeechModel::server_turns(vec![
        say("tell me a story", "Once upon a time."),
        say("stop", "Stopped."),
    ]));
    let mut conversation = Conversation::open(
        &mut model,
        nova_config(),
        &host,
        Options::default(),
        Box::new(|| json!({"local_time": "12:34"})),
    )
    .expect("the fake devices and the mock session open");

    capture(&host, 100, -0.25);
    capture(&host, 100, 0.0);
    let mut notes = Vec::new();
    for _ in 0..50 {
        notes.extend(conversation.step());
        if notes.iter().any(|note| {
            matches!(
                note,
                Note::Ended {
                    how: Ending::Completed,
                    ..
                }
            )
        }) {
            break;
        }
    }
    let _ = host.pull(TEN_MS_FRAMES).expect("the speaker runs");
    assert!(
        conversation.speaker().queued() > 0,
        "the rest of the answer still plays: {notes:#?}"
    );

    capture(&host, 40, -0.25);
    let mut after = Vec::new();
    for _ in 0..5 {
        after.extend(conversation.step());
    }

    assert!(
        after
            .iter()
            .any(|note| matches!(note, Note::BargeIn { .. })),
        "the operator spoke over the answer and playout went on, {} samples still queued: \
         {after:#?}",
        conversation.speaker().queued()
    );
    assert_eq!(conversation.speaker().queued(), 0, "nothing more plays");
}
