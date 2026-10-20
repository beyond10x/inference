//! The testkit's duplex mock as the podcast's model: each session it opens speaks the next
//! scripted line, and every configuration it was opened with is kept.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use inference_audio_device::fake::FakeHost;
use inference_duplex::inference_audio::{AudioFrame, SampleClockPosition};
use inference_duplex::inference_core::model::Adapter;
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle,
};
use inference_testkit::duplex::mock::{MockSpeechModel, MockSpeechSession, Reply, ScriptedTurn};
use repo_podcast::{Event, Host, Settings, Show, default_hosts};

/// The mock with one scripted line per session, in order.
pub struct Scripted {
    template: MockSpeechModel,
    lines: VecDeque<String>,
    pub configs: Rc<RefCell<Vec<SessionConfig>>>,
}

impl Scripted {
    /// Lines `<name> line <n>` for `turns` turns, the hosts alternating from the first.
    pub fn alternating(turns: usize) -> Self {
        let hosts = default_hosts();
        let lines = (0..turns)
            .map(|turn| format!("{} line {turn}", hosts[turn % 2].name))
            .collect();
        Self {
            template: MockSpeechModel::native(Vec::new()),
            lines,
            configs: Rc::default(),
        }
    }
}

/// A session of [`Scripted`]: the mock's own, except that asking for a response first ends the
/// mock's (empty) user turn, which is what makes the mock speak its scripted reply.
pub struct Line(MockSpeechSession);

impl NativeSpeechModel for Scripted {
    type Session = Line;

    fn adapter(&self) -> &Adapter {
        self.template.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        self.template.capabilities()
    }

    fn open(&mut self, config: SessionConfig) -> Result<Line, OpenError> {
        self.configs.borrow_mut().push(config.clone());
        let line = self
            .lines
            .pop_front()
            .unwrap_or_else(|| "nothing more".to_owned());
        let mut mock = MockSpeechModel::native(vec![ScriptedTurn {
            heard: "your turn".to_owned(),
            reply: Reply::Say(line),
        }]);
        mock.open(config).map(Line)
    }
}

impl SpeechSessionHandle for Line {
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
        self.0.commit_input()?;
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
        self.0.next_event(wait)
    }
}

/// The brief the tests give the hosts.
pub const BRIEF: &str = "inference is a Rust library of contracts for AI inference.";

/// Settings for a show of `limit` audio on the mock: reads that never wait.
pub fn settings(limit: Duration) -> Settings {
    Settings {
        limit,
        wait: Duration::ZERO,
        ..Settings::default()
    }
}

/// Opens a show of `limit` audio on `model` and the fake laptop host.
pub fn open(model: Scripted, limit: Duration) -> (Show<Scripted>, FakeHost) {
    let host = FakeHost::laptop();
    let hosts: [Host; 2] = default_hosts();
    let show = Show::open(
        model,
        hosts,
        BRIEF.to_owned(),
        &host,
        settings(limit),
        |_| None,
    )
    .expect("the fake speaker opens");
    (show, host)
}

/// Runs the show to its end, the fake device playing 10 ms after each step; every event in
/// order.
pub fn run(show: &mut Show<Scripted>, host: &FakeHost) -> Vec<Event> {
    let mut events = Vec::new();
    for _ in 0..20_000 {
        events.extend(show.step());
        if show.is_over() {
            return events;
        }
        let _ = host.pull(480);
    }
    panic!("the show did not end: {events:#?}");
}
