//! Adversary pass 1: GPT-Live barge-in on the user's transcript, with the events the GPT-Live
//! binding records.
//!
//! The session here records what `adapters/inference-openai/src/live_duplex.rs` records for each
//! server event, through the same ledger calls and nothing else: an input transcript fragment is
//! one `InputTranscribed`, at any time (`record`, `LiveEvent::InputTranscript`), preceded by one
//! `SpeechStarted` when its `start_ms` lies inside what the response in progress has spoken, once
//! per response (`speech_over`); an output transcript fragment or a voiced output frame begins a
//! response when none is in progress (`begin`, `output`); silent output is delivered on the
//! response in progress until the silence gap completes it; a backend function call is proposed on
//! the response in progress. An output transcript fragment after speech over the response, which
//! here always starts after that speech ended, completes the cut response and begins a new one
//! for the model's next words (`resumed`). The test pushes the server's events and steps the
//! conversation.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::num::{NonZeroU16, NonZeroU32};
use std::rc::Rc;
use std::time::{Duration, Instant};

use inference_audio_device::fake::FakeHost;
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_core::model::Adapter;
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle, SpeechSessionId,
};
use inference_testkit::duplex::mock::MockSpeechModel;
use realtime_voice::{Conversation, Note, Options, TIME_TOOL, openai_live_config};
use serde_json::json;

/// Samples of one 100 ms output frame at 24 kHz, as the live sessions sent them.
const FRAME: usize = 2_400;

/// Device frames of one 100 ms output frame on the fake laptop's 48 kHz.
const FRAME_ON_DEVICE: usize = 4_800;

/// One server event, as the binding reads it.
#[derive(Debug, Clone)]
enum Server {
    /// `session.input_transcript.delta` starting outside what the response in progress has
    /// spoken.
    Heard(&'static str),
    /// `session.input_transcript.delta` starting inside it, the first such of the response.
    HeardOver(&'static str),
    /// `session.output_transcript.delta`.
    Said(&'static str),
    /// `session.output_audio.delta` of 100 ms at or above the silence peak.
    Voiced,
    /// `session.output_audio.delta` of 100 ms below it.
    Silent,
    /// A backend `response.output_item.done` holding a call of the demo tool.
    Call,
}

type Feed = Rc<RefCell<VecDeque<Server>>>;

struct Live {
    adapter: Adapter,
    capabilities: SpeechCapabilities,
    feed: Feed,
}

struct LiveSession {
    ledger: SessionLedger,
    feed: Feed,
    current: Option<ResponseId>,
    emitted: u64,
    /// Speech was reported over the response in progress and the model has not spoken since.
    interrupted: bool,
    cancels: Vec<ResponseId>,
    truncations: Vec<ResponseId>,
    results: Vec<SpeechCallId>,
}

impl NativeSpeechModel for Live {
    type Session = LiveSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SessionConfig) -> Result<LiveSession, OpenError> {
        let (ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config,
            Instant::now(),
        )?;
        Ok(LiveSession {
            ledger,
            feed: Rc::clone(&self.feed),
            current: None,
            emitted: 0,
            interrupted: false,
            cancels: Vec::new(),
            truncations: Vec::new(),
            results: Vec::new(),
        })
    }
}

fn pcm24() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

impl LiveSession {
    /// `begin` of the binding: the response in progress, created when none is.
    fn begin(&mut self) -> ResponseId {
        if let Some(id) = self.current {
            return id;
        }
        let id = ResponseId::generate();
        self.ledger
            .response_created(id, Instant::now())
            .expect("no response in progress");
        self.current = Some(id);
        self.emitted = 0;
        id
    }

    fn audio(&mut self, id: ResponseId, level: i16) {
        let data: Vec<u8> = level
            .to_le_bytes()
            .into_iter()
            .cycle()
            .take(FRAME * 2)
            .collect();
        let frame = AudioFrame {
            format: pcm24(),
            position: SampleClockPosition::new(self.emitted).unwrap(),
            sample_count: FRAME as u32,
            data,
        };
        self.ledger
            .output_audio(id, frame, Instant::now())
            .expect("audio of the response in progress");
        self.emitted += FRAME as u64;
    }

    fn record(&mut self, event: Server) {
        let now = Instant::now();
        match event {
            Server::Heard(text) => {
                let item = self.ledger.assign_item();
                self.ledger
                    .input_transcribed(item, text.to_owned(), now)
                    .expect("open");
            }
            Server::HeardOver(text) => {
                self.interrupted = self.current.is_some();
                self.ledger
                    .speech_started(SampleClockPosition::new(0).unwrap(), now)
                    .expect("open");
                let item = self.ledger.assign_item();
                self.ledger
                    .input_transcribed(item, text.to_owned(), now)
                    .expect("open");
            }
            Server::Said(text) => {
                if std::mem::take(&mut self.interrupted)
                    && let Some(cut) = self.current.take()
                {
                    self.ledger
                        .response_completed(cut, now)
                        .expect("the cut response is in progress");
                }
                let id = self.begin();
                self.ledger
                    .output_transcript(id, text.to_owned(), now)
                    .expect("transcript of the response in progress");
            }
            Server::Voiced => {
                let id = self.begin();
                self.audio(id, 3_000);
            }
            Server::Silent => {
                if let Some(id) = self.current {
                    self.audio(id, 12);
                }
            }
            Server::Call => {
                let id = self.begin();
                self.ledger
                    .tool_call(SpeechCallId::new("call_1"), id, TIME_TOOL, json!({}), now)
                    .expect("the demo tool is offered");
            }
        }
    }
}

impl SpeechSessionHandle for LiveSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        self.ledger.accept_input(frame)
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.ledger
            .commit_input(inference_duplex::InputItemId::new("turn-1"), Instant::now())
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.ledger.request_response(Instant::now())
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.cancels.push(response_id);
        self.ledger.cancel(response_id)
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.truncations.push(response_id);
        self.ledger.truncate(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        self.ledger.answer(call_id, result)?;
        self.results.push(call_id.clone());
        Ok(())
    }

    fn close(&mut self) -> Result<(), SessionError> {
        self.ledger.close()
    }

    fn next_event(&mut self, _wait: Duration) -> Option<SessionEvent> {
        loop {
            let next = self.feed.borrow_mut().pop_front();
            match next {
                Some(event) => self.record(event),
                None => break,
            }
        }
        self.ledger.next_event()
    }
}

struct Run {
    host: FakeHost,
    feed: Feed,
    conversation: Conversation<LiveSession>,
}

fn run_with(capabilities: SpeechCapabilities, config: SessionConfig) -> Run {
    let host = FakeHost::laptop();
    let feed: Feed = Rc::default();
    let mut model = Live {
        adapter: MockSpeechModel::native(Vec::new()).adapter().clone(),
        capabilities,
        feed: Rc::clone(&feed),
    };
    let conversation = Conversation::open(
        &mut model,
        config,
        &host,
        Options::default(),
        Box::new(|| json!({"local_time": "12:34"})),
    )
    .expect("the fake devices and the session open");
    Run {
        host,
        feed,
        conversation,
    }
}

fn run() -> Run {
    run_with(
        inference_openai::live_duplex::capabilities(),
        openai_live_config(),
    )
}

impl Run {
    /// The server sends `events`; the conversation takes one step.
    fn server(&mut self, events: &[Server]) -> Vec<Note> {
        self.feed.borrow_mut().extend(events.iter().cloned());
        self.conversation.step()
    }

    fn queued(&self) -> usize {
        self.conversation.speaker().queued()
    }

    fn in_progress(&self) -> Option<ResponseId> {
        self.conversation.session().ledger().in_progress()
    }
}

fn barge_ins(notes: &[Note]) -> usize {
    notes
        .iter()
        .filter(|note| matches!(note, Note::BargeIn { .. }))
        .count()
}

fn refused(notes: &[Note]) -> bool {
    notes.iter().any(|note| matches!(note, Note::Refused(_)))
}

/// The transcript of the user's question streams in fragments and lags the audio it transcribes;
/// GPT-Live decides by itself when to answer. A fragment of the question delivered after the
/// answer's first audio is no speech over the answer, and nothing in it says otherwise: the
/// binding drops its `start_ms` and `end_ms`. The answer keeps playing, all of it.
#[test]
fn a_trailing_fragment_of_the_question_does_not_cut_its_answer() {
    let mut run = run();
    let mut notes = run.server(&[
        Server::Heard("What is the"),
        Server::Said(" Paris is"),
        Server::Voiced,
        Server::Voiced,
    ]);
    let answer = run.in_progress().expect("the answer is in progress");
    assert_eq!(run.queued(), 2 * FRAME, "two frames of the answer queued");

    notes.extend(run.server(&[
        Server::Heard(" capital of France?"),
        Server::Said(" the capital."),
        Server::Voiced,
    ]));
    assert_eq!(
        run.in_progress(),
        Some(answer),
        "the model went on with the same answer"
    );
    assert_eq!(
        run.queued(),
        3 * FRAME,
        "every frame of the answer is still to play, none dropped: {notes:#?}"
    );
    assert_eq!(barge_ins(&notes), 0, "no barge-in: {notes:#?}");
}

/// A response's trailing silence is delivered until the silence gap completes it
/// (`live_duplex.rs`, `output`), so the speaker holds it when the user's next question starts
/// right after the answer. That is a turn, not an interruption: no barge-in is reported.
#[test]
fn the_next_question_over_the_answers_trailing_silence_is_no_barge_in() {
    let mut run = run();
    let mut notes = run.server(&[
        Server::Heard("Hi"),
        Server::Said(" Hello."),
        Server::Voiced,
        Server::Silent,
        Server::Silent,
    ]);
    let _ = run.host.pull(FRAME_ON_DEVICE).expect("the speaker runs");
    assert_eq!(
        run.queued(),
        2 * FRAME,
        "the answer was heard; its silence is queued"
    );

    notes.extend(run.server(&[Server::Heard(" What time")]));
    assert_eq!(
        barge_ins(&notes),
        0,
        "the user spoke after the answer, over silence: {notes:#?}"
    );
}

/// A transcript while the speaker holds nothing stops nothing and reports nothing.
#[test]
fn a_transcript_over_an_idle_speaker_reports_no_barge_in() {
    let mut run = run();
    let notes = run.server(&[Server::Heard("hello")]);
    assert!(
        notes.contains(&Note::Heard("hello".to_owned())),
        "{notes:#?}"
    );
    assert_eq!(barge_ins(&notes), 0, "{notes:#?}");
    assert!(!refused(&notes), "{notes:#?}");
}

/// Fragment after fragment over one answer: one stop, nothing cancelled or truncated, and audio
/// the model goes on to send still plays.
#[test]
fn repeated_fragments_stop_the_answer_once_and_later_audio_plays() {
    let mut run = run();
    let mut notes = run.server(&[
        Server::Heard("tell me a story"),
        Server::Said(" Once upon"),
        Server::Voiced,
        Server::Voiced,
    ]);
    let answer = run.in_progress().expect("in progress");
    notes.extend(run.server(&[
        Server::HeardOver("wait"),
        Server::Heard(" a"),
        Server::Heard(" moment"),
    ]));
    assert_eq!(barge_ins(&notes), 1, "one stop: {notes:#?}");
    assert!(notes.contains(&Note::BargeIn {
        response_id: Some(answer),
        heard: Some(SampleClockPosition::new(0).unwrap()),
    }));
    assert_eq!(run.queued(), 0);
    notes.extend(run.server(&[Server::Said(" Sure."), Server::Voiced]));
    assert_eq!(run.queued(), FRAME, "what the model says next plays");
    let session = run.conversation.session();
    assert!(session.cancels.is_empty() && session.truncations.is_empty());
    assert!(!refused(&notes), "{notes:#?}");
}

/// A transcript during the tool round trip stops the filler being played; the call is answered
/// once and the answer that follows plays.
#[test]
fn a_transcript_during_the_tool_round_trip_leaves_the_answer_to_play() {
    let mut run = run();
    let mut notes = run.server(&[
        Server::Heard("what time is it"),
        Server::Call,
        Server::Said(" Let me check."),
        Server::Voiced,
    ]);
    assert_eq!(
        run.conversation.session().results,
        vec![SpeechCallId::new("call_1")]
    );
    assert_eq!(run.queued(), FRAME);
    notes.extend(run.server(&[Server::HeardOver("hmm")]));
    assert_eq!(run.queued(), 0, "the filler stopped");
    notes.extend(run.server(&[
        Server::Said(" It is 12:34."),
        Server::Voiced,
        Server::Voiced,
    ]));
    assert_eq!(run.queued(), 2 * FRAME, "the answer plays: {notes:#?}");
    assert_eq!(run.conversation.session().results.len(), 1, "answered once");
    assert!(!refused(&notes), "{notes:#?}");
}

/// The `auto_interrupt` term of the condition: a model without truncation whose barge-in is the
/// caller's (`auto_interrupt` off) is stopped on the speech start alone, never on a transcript.
/// No demo configuration has both, so nothing else in the suite reads the term.
#[test]
fn with_auto_interrupt_off_a_transcript_stops_nothing() {
    let mut capabilities = inference_openai::live_duplex::capabilities();
    capabilities.manual_interrupt = true;
    capabilities.client_cancel = true;
    let mut config = openai_live_config();
    config.auto_interrupt = false;
    let mut run = run_with(capabilities, config);
    let mut notes = run.server(&[Server::Said(" Once upon"), Server::Voiced]);
    notes.extend(run.server(&[Server::Heard("hmm")]));
    assert_eq!(run.queued(), FRAME, "{notes:#?}");
    assert_eq!(barge_ins(&notes), 0, "{notes:#?}");
    assert!(run.conversation.session().cancels.is_empty());
}
