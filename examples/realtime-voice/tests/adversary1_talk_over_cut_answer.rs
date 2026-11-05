//! Adversary pass on `story:long-talk-over-does-not-resume-the-cut-answer`, the native path.
//!
//! Two attacks on what `talk_over.rs` holds:
//!
//! * A session that records what the OpenAI Realtime binding records with `auto_interrupt` on
//!   (`interrupt_response` in the session's turn detection): the server reports the caller's
//!   speech, then still delivers audio deltas of the response before its `response.done` with
//!   status `cancelled`. Those deltas reach the ledger while the response is in progress, so
//!   the ledger takes them, and `Conversation` has itself truncated that response at the
//!   barge-in. Audio of a response `Conversation` truncated is the cut answer, so it must not
//!   play.
//! * A session that resumes the cut answer, written as a test-side wrapper of the testkit mock:
//!   it delivers the frames of the cut response the speaker had not played once more, either
//!   just before the next answer or right behind it. `talk_over.rs` reads the ticks of the
//!   talk-over only up to the first tick in which the next answer played; these cases read on
//!   until the speaker has played everything out, and show which resume that window sees.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use inference_audio_device::fake::FakeHost;
use inference_duplex::inference_audio::{AudioFrame, SampleClockPosition};
use inference_duplex::inference_core::model::Adapter;
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, OutputAudio, ResponseId, SessionConfig, SessionError,
    SessionEvent, SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle,
    SpeechSessionId,
};
use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn};
use realtime_voice::{Conversation, Note, Options, nova_config, openai_config, openai_live_config};
use serde_json::json;

// ---------------------------------------------------------------------------------------------
// The Realtime binding's record with `auto_interrupt` on.

/// Samples of one 100 ms output delta at 24 kHz.
const DELTA: usize = 2_400;

/// Device frames of one 100 ms delta on the fake laptop's 48 kHz speaker.
const DELTA_ON_DEVICE: usize = 4_800;

/// The level of every sample of the answer's audio.
const VOICED: i16 = 3_000;

/// One server event, as `adapters/inference-openai/src/duplex.rs` records it.
#[derive(Debug, Clone, Copy)]
enum Server {
    /// `response.created`: a new response in progress.
    Created,
    /// `response.output_audio.delta` of the response in progress, 100 ms.
    Audio,
    /// `input_audio_buffer.speech_started`.
    SpeechStarted,
    /// `response.done` with status `cancelled`.
    Cancelled,
}

type Feed = Rc<RefCell<VecDeque<Server>>>;

struct Realtime {
    adapter: Adapter,
    capabilities: SpeechCapabilities,
    feed: Feed,
}

struct RealtimeSession {
    ledger: SessionLedger,
    feed: Feed,
    current: Option<ResponseId>,
    emitted: u64,
    truncated: Vec<(ResponseId, SampleClockPosition)>,
    refused_audio: usize,
}

impl NativeSpeechModel for Realtime {
    type Session = RealtimeSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SessionConfig) -> Result<RealtimeSession, OpenError> {
        let (ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config,
            Instant::now(),
        )?;
        Ok(RealtimeSession {
            ledger,
            feed: Rc::clone(&self.feed),
            current: None,
            emitted: 0,
            truncated: Vec::new(),
            refused_audio: 0,
        })
    }
}

impl RealtimeSession {
    fn record(&mut self, event: Server) {
        let now = Instant::now();
        match event {
            Server::Created => {
                let id = ResponseId::generate();
                self.ledger
                    .response_created(id, now)
                    .expect("no response in progress");
                self.current = Some(id);
                self.emitted = 0;
            }
            Server::Audio => {
                let Some(id) = self.current else {
                    return;
                };
                let frame = AudioFrame {
                    format: *self.ledger.session().output_format(),
                    position: SampleClockPosition::new(self.emitted).expect("a position"),
                    sample_count: u32::try_from(DELTA).expect("a delta"),
                    data: VOICED
                        .to_le_bytes()
                        .into_iter()
                        .cycle()
                        .take(DELTA * 2)
                        .collect(),
                };
                // duplex.rs `ServerEvent::AudioDelta`: what the ledger takes is delivered, what
                // it refuses is dropped.
                match self.ledger.output_audio(id, frame, now) {
                    Ok(()) => self.emitted += DELTA as u64,
                    Err(refusal) => {
                        self.refused_audio += 1;
                        self.ledger.dropped(refusal);
                    }
                }
            }
            Server::SpeechStarted => {
                self.ledger
                    .speech_started(SampleClockPosition::new(0).expect("0"), now)
                    .expect("open");
            }
            Server::Cancelled => {
                if let Some(id) = self.current.take()
                    && let Err(refusal) = self.ledger.response_cancelled(id, now)
                {
                    // The caller's cancellation already recorded is not recorded again.
                    self.ledger.dropped(refusal);
                }
            }
        }
    }
}

impl SpeechSessionHandle for RealtimeSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        self.ledger.accept_input(frame)
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.ledger.commit_input(Instant::now())
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.ledger.request_response(Instant::now())
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.ledger.cancel(response_id)
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.truncated.push((response_id, heard));
        self.ledger.truncate(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        self.ledger.answer(call_id, result)
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

/// What a barge-in over a Realtime session left playing.
#[derive(Debug)]
struct InFlight {
    /// The responses `Conversation` truncated at the barge-in.
    truncated: Vec<(ResponseId, SampleClockPosition)>,
    /// Samples the speaker queued once the deltas sent after the speech start were handled.
    queued: usize,
    /// Device samples of the answer's level played in the 100 ms after that.
    voiced: usize,
    /// Deltas the ledger refused.
    refused: usize,
    notes: Vec<Note>,
}

/// An answer of five 100 ms deltas plays for 100 ms; the caller starts speaking over it; the
/// server then sends two more deltas of the response before it cancels it.
fn barge_in_with_deltas_in_flight(auto_interrupt: bool) -> InFlight {
    let host = FakeHost::laptop();
    let feed = Feed::default();
    let mut model = Realtime {
        adapter: MockSpeechModel::native(Vec::new()).adapter().clone(),
        capabilities: inference_openai::duplex::capabilities(),
        feed: Rc::clone(&feed),
    };
    let config = SessionConfig {
        auto_interrupt,
        ..openai_config()
    };
    let mut conversation = Conversation::open(
        &mut model,
        config,
        &host,
        Options::default(),
        Box::new(|| json!({"local_time": "12:34"})),
    )
    .expect("the fake devices and the session open");
    let mut notes = Vec::new();
    let mut server = |events: &[Server], conversation: &mut Conversation<RealtimeSession>| {
        feed.borrow_mut().extend(events.iter().copied());
        notes.extend(conversation.step());
    };

    server(
        &[
            Server::Created,
            Server::Audio,
            Server::Audio,
            Server::Audio,
            Server::Audio,
            Server::Audio,
        ],
        &mut conversation,
    );
    let _ = host.pull(DELTA_ON_DEVICE).expect("the speaker runs");
    assert!(
        conversation.speaker().queued() > 0,
        "the answer is still playing when the caller starts speaking"
    );
    server(&[Server::SpeechStarted], &mut conversation);
    assert_eq!(
        conversation.speaker().queued(),
        0,
        "the barge-in stops playout"
    );
    server(&[Server::Audio, Server::Audio], &mut conversation);
    let queued = conversation.speaker().queued();
    let pulled = host.pull(DELTA_ON_DEVICE).expect("the speaker runs");
    let level = f32::from(VOICED) / 32_768.0;
    let voiced = pulled
        .iter()
        .filter(|sample| (**sample - level).abs() < level / 4.0)
        .count();
    server(&[Server::Cancelled], &mut conversation);
    InFlight {
        truncated: conversation.session().truncated.clone(),
        queued,
        voiced,
        refused: conversation.session().refused_audio,
        notes,
    }
}

/// With `auto_interrupt` on, the Realtime server cancels the response itself, and the deltas it
/// sent before that reach the binding after `speech_started` (`duplex.rs` records the speech
/// start and nothing else, and takes every delta while the response is in progress).
/// `Conversation` truncated that response at the barge-in, so what comes of it after is the cut
/// sentence, and nothing of it may play.
#[test]
fn audio_of_the_response_conversation_truncated_at_the_barge_in_does_not_play_after_it() {
    let seen = barge_in_with_deltas_in_flight(true);
    assert!(
        seen.notes
            .iter()
            .any(|note| matches!(note, Note::BargeIn { .. })),
        "the speech start stops playout: {seen:#?}"
    );
    assert!(
        !seen.truncated.is_empty(),
        "the barge-in truncates the answer: {seen:#?}"
    );
    assert_eq!(seen.refused, 0, "the ledger takes the deltas in flight");
    assert_eq!(
        (seen.queued, seen.voiced),
        (0, 0),
        "the deltas of the response truncated at the barge-in are queued and played after it: \
         {seen:#?}"
    );
    assert_eq!(
        seen.truncated.len(),
        1,
        "the answer is truncated once, where the barge-in stopped it, not again where the deltas \
         played after it stopped: {seen:#?}"
    );
}

/// With `auto_interrupt` off, the demo's OpenAI session, `Conversation` cancels the response at
/// the speech start, so the ledger refuses the deltas in flight and nothing of them plays.
#[test]
fn with_auto_interrupt_off_the_deltas_in_flight_are_refused_and_never_play() {
    let seen = barge_in_with_deltas_in_flight(false);
    assert_eq!(seen.refused, 2, "the ledger refuses the deltas: {seen:#?}");
    assert_eq!((seen.queued, seen.voiced), (0, 0), "{seen:#?}");
}

// ---------------------------------------------------------------------------------------------
// A session that resumes the cut answer, over the testkit mock.

/// Interleaved stereo samples of 10 ms at 48 kHz, the fake laptop's microphone.
const TEN_MS: usize = 480 * 2;

/// One tick of the sample clock.
const TICK_MS: usize = 20;

/// Device frames of one tick at 48 kHz.
const TICK_FRAMES: usize = 960;

/// Speech, as the mock hears it.
const SPEECH: f32 = -0.25;

/// A tenth of full scale: the `n`th response's samples are `n` times this.
const MARK: i16 = 3277;

const QUESTION: &str = "tell me a long story";
const ANSWER: &str = "Once upon a time there was a ship.";
const TALK_OVER: &str = "stop, I want the weather instead";
const TALK_OVER_REPLY: &str = "You said: {heard}.";

/// Ticks of silence and an idle speaker that end a play-out.
const IDLE: usize = 25;

type MockSession = <MockSpeechModel as NativeSpeechModel>::Session;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    OpenAi,
    GptLive,
    Nova,
}

const PROVIDERS: [Provider; 3] = [Provider::OpenAi, Provider::GptLive, Provider::Nova];

impl Provider {
    fn model(self, script: Vec<ScriptedTurn>) -> MockSpeechModel {
        match self {
            Self::OpenAi | Self::GptLive => MockSpeechModel::native(script),
            Self::Nova => MockSpeechModel::server_turns(script),
        }
    }

    fn config(self) -> SessionConfig {
        match self {
            Self::OpenAi => openai_config(),
            Self::GptLive => openai_live_config(),
            Self::Nova => nova_config(),
        }
    }
}

/// When the mutant session delivers the cut answer's unplayed frames once more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resume {
    /// Never: the session as the mock is.
    Never,
    /// Just before the next answer's `ResponseCreated`.
    BeforeTheNextAnswer,
    /// Right after the next answer's `ResponseCompleted`.
    BehindTheNextAnswer,
}

struct Resuming {
    inner: MockSpeechModel,
    resume: Resume,
    heard: Rc<Cell<Option<u64>>>,
}

impl NativeSpeechModel for Resuming {
    type Session = ResumingSession;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        self.inner.capabilities()
    }

    fn open(&mut self, config: SessionConfig) -> Result<ResumingSession, OpenError> {
        self.inner.open(config).map(|inner| ResumingSession {
            inner,
            resume: self.resume,
            heard: Rc::clone(&self.heard),
            order: Vec::new(),
            cut: Vec::new(),
            out: VecDeque::new(),
            resumed: false,
        })
    }
}

/// The mock's session, each response's audio marked by its ordinal, and the frames of the
/// first response kept to be resumed.
struct ResumingSession {
    inner: MockSession,
    resume: Resume,
    /// Where the listener stopped hearing the cut answer, as `Note::BargeIn` said.
    heard: Rc<Cell<Option<u64>>>,
    order: Vec<ResponseId>,
    cut: Vec<OutputAudio>,
    out: VecDeque<SessionEvent>,
    resumed: bool,
}

impl ResumingSession {
    /// The frames of the cut answer the listener had not heard, once.
    fn unplayed(&mut self) -> Vec<SessionEvent> {
        if self.resumed {
            return Vec::new();
        }
        self.resumed = true;
        let heard = self.heard.get().unwrap_or(0);
        self.cut
            .iter()
            .filter(|audio| {
                audio.frame.position.get() + u64::from(audio.frame.sample_count) > heard
            })
            .cloned()
            .map(SessionEvent::OutputAudio)
            .collect()
    }
}

fn mark(frame: &mut AudioFrame, n: usize) {
    let sample = MARK * i16::try_from(n).expect("a few responses");
    for pair in frame.data.as_chunks_mut::<2>().0 {
        *pair = sample.to_le_bytes();
    }
}

impl SpeechSessionHandle for ResumingSession {
    fn ledger(&self) -> &SessionLedger {
        self.inner.ledger()
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        self.inner.append_audio(frame)
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.inner.commit_input()
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.inner.create_response()
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.inner.cancel_response(response_id)
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.inner.truncate_output(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        self.inner.submit_tool_result(call_id, result)
    }

    fn close(&mut self) -> Result<(), SessionError> {
        self.inner.close()
    }

    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent> {
        if let Some(event) = self.out.pop_front() {
            return Some(event);
        }
        let mut event = self.inner.next_event(wait)?;
        let mut before = false;
        let mut behind = false;
        match &mut event {
            SessionEvent::ResponseCreated(created) => {
                self.order.push(created.response_id);
                before = self.order.len() == 2 && self.resume == Resume::BeforeTheNextAnswer;
            }
            SessionEvent::OutputAudio(audio) => {
                let n = self
                    .order
                    .iter()
                    .position(|created| *created == audio.response_id)
                    .expect("audio of a response created before it")
                    + 1;
                mark(&mut audio.frame, n);
                if n == 1 {
                    self.cut.push(audio.clone());
                }
            }
            SessionEvent::ResponseCompleted(done) => {
                behind = self.order.get(1) == Some(&done.response_id)
                    && self.resume == Resume::BehindTheNextAnswer;
            }
            _ => {}
        }
        if before {
            let resumed = self.unplayed();
            self.out.extend(resumed);
            self.out.push_back(event);
            return self.out.pop_front();
        }
        if behind {
            let resumed = self.unplayed();
            self.out.extend(resumed);
        }
        Some(event)
    }
}

fn mark_of(sample: f32) -> Option<usize> {
    let tenth = f32::from(MARK) / 32_768.0;
    (1..10).find(|n| (sample - tenth * *n as f32).abs() < tenth / 5.0)
}

/// The responses whose marks the pulled samples carry, each once, in runs of eight or more.
fn marks(pulled: &[f32]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    for mark in pulled.iter().map(|sample| mark_of(*sample)) {
        run = match (run, mark) {
            (Some((n, length)), Some(m)) if n == m => Some((n, length + 1)),
            (_, Some(m)) => Some((m, 1)),
            (_, None) => None,
        };
        if let Some((n, 8)) = run
            && !found.contains(&n)
        {
            found.push(n);
        }
    }
    found.sort_unstable();
    found
}

struct Call {
    host: FakeHost,
    conversation: Conversation<ResumingSession>,
    heard: Rc<Cell<Option<u64>>>,
    played: Vec<Vec<usize>>,
}

impl Call {
    fn open(provider: Provider, resume: Resume) -> Self {
        let host = FakeHost::laptop();
        let heard = Rc::new(Cell::new(None));
        let script = vec![
            ScriptedTurn {
                heard: QUESTION.to_owned(),
                reply: Reply::Say(ANSWER.to_owned()),
            },
            ScriptedTurn {
                heard: TALK_OVER.to_owned(),
                reply: Reply::Say(TALK_OVER_REPLY.to_owned()),
            },
        ];
        let mut model = Resuming {
            inner: provider.model(script),
            resume,
            heard: Rc::clone(&heard),
        };
        let conversation = Conversation::open(
            &mut model,
            provider.config(),
            &host,
            Options::default(),
            Box::new(|| json!({"local_time": "12:34"})),
        )
        .expect("the fake devices and the mock session open");
        Self {
            host,
            conversation,
            heard,
            played: Vec::new(),
        }
    }

    fn now(&self) -> usize {
        self.played.len()
    }

    fn tick(&mut self, sample: f32) {
        for _ in 0..TICK_MS / 10 {
            assert!(self.host.feed(&[sample; TEN_MS]), "the microphone runs");
        }
        for note in self.conversation.step() {
            if let Note::BargeIn {
                response_id: Some(_),
                heard: Some(heard),
            } = note
            {
                self.heard.set(Some(heard.get()));
            }
        }
        let pulled = self.host.pull(TICK_FRAMES).expect("the speaker runs");
        self.played.push(marks(&pulled));
    }

    fn speak(&mut self, millis: usize) {
        for _ in 0..millis / TICK_MS {
            self.tick(SPEECH);
        }
    }

    fn has_played(&self, n: usize) -> bool {
        self.played.iter().any(|ticks| ticks.contains(&n))
    }

    fn pause_until(&mut self, millis: usize, what: &str, done: impl Fn(&Self) -> bool) {
        for _ in 0..millis / TICK_MS {
            self.tick(0.0);
            if done(self) {
                return;
            }
        }
        panic!("{what}: not within {millis} ms");
    }

    /// Silence at the microphone until the speaker has queued and played nothing for
    /// [`IDLE`] ticks in a row, at most `millis` of it.
    fn play_out(&mut self, millis: usize) {
        let mut idle = 0;
        for _ in 0..millis / TICK_MS {
            self.tick(0.0);
            let silent = self.played.last().is_some_and(Vec::is_empty);
            if silent && self.conversation.speaker().queued() == 0 {
                idle += 1;
                if idle == IDLE {
                    return;
                }
            } else {
                idle = 0;
            }
        }
        panic!("the speaker plays out: not within {millis} ms");
    }
}

/// What a talk-over showed, read over the window `talk_over.rs` reads and to the end.
#[derive(Debug)]
struct Seen {
    provider: Provider,
    talk_ms: usize,
    resume: Resume,
    /// Ticks from the talk-over's first, up to the first in which the next answer played, in
    /// which the cut answer played: what `talk_over.rs` reads.
    cut_in_window: Vec<usize>,
    /// The responses played in those ticks, in the order first played.
    after_in_window: Vec<usize>,
    /// Ticks from the talk-over's first to the end of the play-out in which the cut answer
    /// played.
    cut_to_the_end: Vec<usize>,
    /// The responses played from the talk-over's first tick to the end, in the order first
    /// played.
    after_to_the_end: Vec<usize>,
}

fn first_played(ticks: &[Vec<usize>]) -> Vec<usize> {
    let mut order = Vec::new();
    for n in ticks.iter().flatten() {
        if !order.contains(n) {
            order.push(*n);
        }
    }
    order
}

fn cut_ticks(ticks: &[Vec<usize>]) -> Vec<usize> {
    ticks
        .iter()
        .enumerate()
        .filter(|(_, played)| played.contains(&1))
        .map(|(at, _)| at)
        .collect()
}

fn talk_over(provider: Provider, talk_ms: usize, resume: Resume) -> Seen {
    let mut call = Call::open(provider, resume);
    call.speak(100);
    call.pause_until(1_000, "the answer plays", |call| call.has_played(1));
    assert!(
        call.conversation.speaker().queued() > 0,
        "{provider:?}: the answer is still playing when the talk-over starts"
    );
    let started = call.now();
    call.speak(talk_ms);
    call.pause_until(2_000, "the next answer plays", |call| call.has_played(2));
    let window = call.now();
    call.play_out(10_000);
    let in_window = &call.played[started..window];
    let to_the_end = &call.played[started..];
    Seen {
        provider,
        talk_ms,
        resume,
        cut_in_window: cut_ticks(in_window),
        after_in_window: first_played(in_window),
        cut_to_the_end: cut_ticks(to_the_end),
        after_to_the_end: first_played(to_the_end),
    }
}

impl Seen {
    fn label(&self) -> String {
        format!(
            "{:?}, a talk-over of {} ms, resume {:?}",
            self.provider, self.talk_ms, self.resume
        )
    }
}

/// The acceptance read to the end: after a talk-over of 6 s or 12 s, no frame of the cut answer
/// plays until the speaker has played everything out, and the next answer is all that plays.
#[test]
fn no_frame_of_the_cut_answer_plays_until_the_speaker_has_played_everything_out() {
    for provider in PROVIDERS {
        for talk_ms in [6_000, 12_000] {
            let seen = talk_over(provider, talk_ms, Resume::Never);
            assert!(
                seen.cut_to_the_end.is_empty(),
                "the cut answer plays again after the talk-over started: {}: {seen:#?}",
                seen.label()
            );
            assert_eq!(
                seen.after_to_the_end,
                [2],
                "only the next answer plays: {}: {seen:#?}",
                seen.label()
            );
        }
    }
}

/// A session that resumes the cut answer just before the next answer: the frames play before
/// the next answer's first tick, so the window `talk_over.rs` reads sees them.
#[test]
fn a_resume_before_the_next_answer_is_seen_inside_the_window_talk_over_reads() {
    for provider in PROVIDERS {
        let seen = talk_over(provider, 6_000, Resume::BeforeTheNextAnswer);
        assert!(
            !seen.cut_in_window.is_empty(),
            "the mutant resumes the cut answer before the next one: {}: {seen:#?}",
            seen.label()
        );
    }
}

/// A session that resumes the cut answer right behind the next answer: `Conversation` queues
/// those frames behind the next answer's, so they play once it has played out. The window
/// `talk_over.rs` reads ends at the next answer's first tick, and sees nothing of them: both its
/// checks (`never_plays_the_cut_answer_again`: no tick of the cut answer, and only the next
/// answer played) hold on this mutant. Read to the end, the cut answer plays again.
#[test]
fn a_resume_behind_the_next_answer_passes_the_window_talk_over_reads_and_plays_after_it() {
    for provider in PROVIDERS {
        let seen = talk_over(provider, 6_000, Resume::BehindTheNextAnswer);
        assert!(
            seen.cut_in_window.is_empty() && seen.after_in_window == [2],
            "the window of talk_over.rs sees nothing of the resume: {}: {seen:#?}",
            seen.label()
        );
        assert!(
            !seen.cut_to_the_end.is_empty() && seen.after_to_the_end == [2, 1],
            "read to the end, the cut answer plays again behind the next answer: {}: {seen:#?}",
            seen.label()
        );
    }
}
