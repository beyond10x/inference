//! The caller talks over an answer that is playing, on the native path
//! (`story:long-talk-over-does-not-resume-the-cut-answer`): [`Conversation`] over the testkit's
//! scripted mock model and the device adapter's fake host, for each barge-in rule of the demo
//! sessions. `auto_interrupt` is on for GPT-Live (the `native` mock) and Nova Sonic (the
//! `server_turns` mock), where the model cuts its own response, and off for the OpenAI Realtime
//! session (the `native` mock), where `Conversation` cancels and truncates.
//!
//! Time is the sample clock: one tick is 20 ms of microphone audio fed to the fake host, one
//! [`Conversation::step`], then 20 ms of speaker audio pulled from it. Nothing sleeps, and every
//! wait is a number of ticks that fails the case when it runs out.
//!
//! Each frame the session delivers is marked by its response: every sample of the `n`th
//! response's audio is set to `n` tenths of full scale (the mock's audio is silence), so the
//! speaker's output says which response was played in each tick.
//!
//! Each case states when playout of the cut answer stops, whether any frame of it is played once
//! the talk-over started, and what the next answer answers: the mock answers the talk-over with
//! a reply naming what it heard, and that reply's response is the next one played.
//!
//! The provider-side cases run a session that goes on streaming the cut response after the
//! caller started speaking, as a provider with audio in flight, or one that does not take the
//! caller's speech as an interruption, does: one more frame of the response each tick for the
//! first half second of the talk-over, before it completes the response. They run with
//! `auto_interrupt` on only. With it off, `Conversation` cancels the response in progress at the
//! speech start and the session's ledger refuses its later output, so no binding delivers any.
//!
//! * The OpenAI Realtime session with `auto_interrupt` on: the server cuts the response itself
//!   and the deltas it had in flight still reach the session; `Conversation` drops them, the
//!   response being cut at the speech start whether or not the speaker still held any of it.
//! * GPT-Live, which claims no truncation, as its binding does: the binding delivers the rest of
//!   the cut sentence on the cut response and opens a new response for the model's next words
//!   (`adapters/inference-openai/src/live_duplex.rs`), so `Conversation` drops the streamed
//!   frames, and `adversary1_gpt_live_transcript.rs` holds that the next words play.
//! * No Nova Sonic case: its binding records the speech start and cancels the response in the
//!   same call at the provider's interruption notice (`adapters/inference-aws/src/duplex.rs`
//!   755-764), and completes it at the user's next transcript (806-811), and the ledger refuses
//!   audio of a response not in progress, so no Nova session delivers such frames.
//!
//! Every case reads on until the speaker has been idle for [`IDLE_TICKS`], so a resume behind
//! the next answer is seen too.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use inference_audio_device::fake::FakeHost;
use inference_duplex::inference_audio::{AudioFrame, SampleClockPosition};
use inference_duplex::inference_core::model::Adapter;
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, OutputAudio, ResponseId, SessionConfig, SessionError,
    SessionEvent, SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle,
};
use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn};
use realtime_voice::{Conversation, Note, Options, nova_config, openai_config, openai_live_config};
use serde_json::json;

type MockSession = <MockSpeechModel as NativeSpeechModel>::Session;

/// Interleaved stereo samples of 10 ms at 48 kHz, the fake laptop's microphone.
const TEN_MS: usize = 480 * 2;

/// One tick of the sample clock, the microphone's frame.
const TICK_MS: usize = 20;

/// Device frames of one tick at 48 kHz, the fake laptop's speaker.
const TICK_FRAMES: usize = 960;

/// Speech, as the mock hears it: a constant sample its recogniser cannot read.
const SPEECH: f32 = -0.25;

/// A tenth of full scale: the `n`th response's samples are `n` times this.
const MARK: i16 = 3277;

/// What the caller asks first.
const QUESTION: &str = "tell me a long story";

/// The answer the caller talks over.
const ANSWER: &str = "Once upon a time there was a ship.";

/// What the caller says during the talk-over, as the mock's script hears it.
const TALK_OVER: &str = "stop, I want the weather instead";

/// The reply to the talk-over: what the model heard during it.
const TALK_OVER_REPLY: &str = "You said: {heard}.";

/// How many frames of the cut response a provider-side case streams after speech started: one a
/// tick, half a second of ticks.
const LATE_FRAMES: usize = 25;

/// Ticks of an idle speaker, after the next answer started playing, that end a case's reading.
const IDLE_TICKS: usize = 25;

/// A demo session and the mock whose barge-in rule it is run with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    /// `openai_config` on the `native` mock: `auto_interrupt` off.
    OpenAi,
    /// `openai_config` with `auto_interrupt` on, on the `native` mock: the server cuts the
    /// response, `Conversation` truncates it.
    OpenAiInterrupting,
    /// `openai_live_config` on the `native` mock claiming no truncation, as the GPT-Live binding
    /// claims none: `auto_interrupt` on.
    GptLive,
    /// `nova_config` on the `server_turns` mock: `auto_interrupt` on, speech reported at the
    /// provider's notice.
    Nova,
}

impl Provider {
    fn model(self, script: Vec<ScriptedTurn>) -> MockSpeechModel {
        match self {
            Self::OpenAi | Self::OpenAiInterrupting | Self::GptLive => {
                MockSpeechModel::native(script)
            }
            Self::Nova => MockSpeechModel::server_turns(script),
        }
    }

    fn config(self) -> SessionConfig {
        match self {
            Self::OpenAi => openai_config(),
            Self::OpenAiInterrupting => SessionConfig {
                auto_interrupt: true,
                ..openai_config()
            },
            Self::GptLive => openai_live_config(),
            Self::Nova => nova_config(),
        }
    }

    /// What `Conversation` is told the model takes: the mock's claims, and for GPT-Live no
    /// truncation, as its binding claims.
    fn capabilities(self, model: &MockSpeechModel) -> SpeechCapabilities {
        let mut capabilities = model.capabilities().clone();
        if self == Self::GptLive {
            capabilities.truncation = inference_openai::live_duplex::capabilities().truncation;
        }
        capabilities
    }
}

/// The mock, its sessions marking each response's audio and, given `late`, streaming that many
/// more frames of the response playing when the caller starts speaking.
struct Marking {
    inner: MockSpeechModel,
    capabilities: SpeechCapabilities,
    late: usize,
}

impl NativeSpeechModel for Marking {
    type Session = MarkingSession;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SessionConfig) -> Result<MarkingSession, OpenError> {
        self.inner.open(config).map(|inner| MarkingSession {
            inner,
            late: self.late,
            order: Vec::new(),
            last_audio: None,
            extended: HashSet::new(),
            held: VecDeque::new(),
            late_frames: VecDeque::new(),
            released: VecDeque::new(),
            paced: false,
        })
    }
}

/// A session of [`Marking`]. With `late` frames to stream, the completion and the usage of the
/// response whose audio came last are held back until the caller next starts speaking; then
/// `late` more frames of it follow, one per step, and only then its completion: a provider still
/// streaming the response when the caller spoke over it, and not cutting it. The mock itself
/// completed the response before the talk-over, so its ledger does not hold the frames made
/// here; `Conversation` reads the ledger only to cancel with `auto_interrupt` off, which the
/// provider-side cases do not run.
struct MarkingSession {
    inner: MockSession,
    late: usize,
    order: Vec<ResponseId>,
    last_audio: Option<OutputAudio>,
    extended: HashSet<ResponseId>,
    held: VecDeque<SessionEvent>,
    late_frames: VecDeque<SessionEvent>,
    released: VecDeque<SessionEvent>,
    paced: bool,
}

impl MarkingSession {
    /// The `n`th response created, from 1.
    fn response(&self, n: usize) -> ResponseId {
        self.order[n - 1]
    }

    fn ordinal(&self, response_id: ResponseId) -> usize {
        self.order
            .iter()
            .position(|created| *created == response_id)
            .expect("audio of a response created before it")
            + 1
    }

    /// Whether the end of `response_id` is held back for the frames still to stream.
    fn holds(&self, response_id: ResponseId) -> bool {
        self.late > 0
            && !self.extended.contains(&response_id)
            && self
                .last_audio
                .as_ref()
                .is_some_and(|last| last.response_id == response_id)
    }

    /// The caller started speaking: the response whose audio came last streams `late` frames
    /// more, continuing its own clock.
    fn stream_on(&mut self) {
        let Some(last) = self.last_audio.clone() else {
            return;
        };
        if self.late == 0 || !self.extended.insert(last.response_id) {
            return;
        }
        let mut frame = last.frame;
        for _ in 0..self.late {
            let next = frame.position.get() + u64::from(frame.sample_count);
            frame.position = SampleClockPosition::new(next).expect("within the clock");
            self.late_frames
                .push_back(SessionEvent::OutputAudio(OutputAudio {
                    response_id: last.response_id,
                    frame: frame.clone(),
                }));
        }
    }
}

/// Sets every sample of `frame` to `n` tenths of full scale.
fn mark(frame: &mut AudioFrame, n: usize) {
    let sample = MARK * i16::try_from(n).expect("a few responses");
    for pair in frame.data.as_chunks_mut::<2>().0 {
        *pair = sample.to_le_bytes();
    }
}

impl SpeechSessionHandle for MarkingSession {
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
        if !self.late_frames.is_empty() {
            if self.paced {
                self.paced = false;
                return None;
            }
            self.paced = true;
            let frame = self.late_frames.pop_front();
            if self.late_frames.is_empty() {
                self.released.extend(self.held.drain(..));
            }
            return frame;
        }
        if let Some(event) = self.released.pop_front() {
            return Some(event);
        }
        loop {
            let mut event = self.inner.next_event(wait)?;
            match &mut event {
                SessionEvent::ResponseCreated(created) => self.order.push(created.response_id),
                SessionEvent::OutputAudio(audio) => {
                    let n = self.ordinal(audio.response_id);
                    mark(&mut audio.frame, n);
                    self.last_audio = Some(audio.clone());
                }
                SessionEvent::ResponseUsageRecorded(recorded)
                    if self.holds(recorded.response_id) =>
                {
                    self.held.push_back(event);
                    continue;
                }
                SessionEvent::ResponseCompleted(done) if self.holds(done.response_id) => {
                    self.held.push_back(event);
                    continue;
                }
                SessionEvent::SpeechStarted(_) => self.stream_on(),
                _ => {}
            }
            return Some(event);
        }
    }
}

/// The response whose mark `sample` carries, if any.
fn mark_of(sample: f32) -> Option<usize> {
    let tenth = f32::from(MARK) / 32_768.0;
    (1..10).find(|n| (sample - tenth * *n as f32).abs() < tenth / 5.0)
}

/// The responses whose marks the pulled samples carry, each once. A mark counts in a run of at
/// least eight samples, so that the speaker's interpolation from one level to the next is not
/// read as a third.
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

/// One tick's record: the notes of its step and the responses whose audio the device played in
/// it.
struct Tick {
    notes: Vec<Note>,
    played: Vec<usize>,
}

/// A conversation on the fake laptop, driven tick by tick.
struct Call {
    host: FakeHost,
    conversation: Conversation<MarkingSession>,
    ticks: Vec<Tick>,
}

impl Call {
    fn open(provider: Provider, late: usize) -> Self {
        let host = FakeHost::laptop();
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
        let inner = provider.model(script);
        let mut model = Marking {
            capabilities: provider.capabilities(&inner),
            inner,
            late,
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
            ticks: Vec::new(),
        }
    }

    /// The ticks run so far.
    fn now(&self) -> usize {
        self.ticks.len()
    }

    /// One tick of `sample` at the microphone.
    fn tick(&mut self, sample: f32) {
        for _ in 0..TICK_MS / 10 {
            assert!(self.host.feed(&[sample; TEN_MS]), "the microphone runs");
        }
        let notes = self.conversation.step();
        let pulled = self.host.pull(TICK_FRAMES).expect("the speaker runs");
        self.ticks.push(Tick {
            notes,
            played: marks(&pulled),
        });
    }

    fn speak(&mut self, millis: usize) {
        for _ in 0..millis / TICK_MS {
            self.tick(SPEECH);
        }
    }

    /// Silence at the microphone until `done` holds, at most `millis` of it.
    fn pause_until(&mut self, millis: usize, what: &str, done: impl Fn(&Self) -> bool) {
        for _ in 0..millis / TICK_MS {
            self.tick(0.0);
            if done(self) {
                return;
            }
        }
        panic!(
            "{what}: not within {millis} ms; notes {:#?}",
            self.notes_from(0)
        );
    }

    fn played(&self, n: usize) -> bool {
        self.ticks.iter().any(|tick| tick.played.contains(&n))
    }

    /// Silence at the microphone until the speaker has played nothing and queued nothing for
    /// [`IDLE_TICKS`] ticks in a row, at most `millis` of it.
    fn play_out(&mut self, millis: usize) {
        let mut idle = 0;
        for _ in 0..millis / TICK_MS {
            self.tick(0.0);
            let quiet = self.ticks.last().is_some_and(|tick| tick.played.is_empty())
                && self.conversation.speaker().queued() == 0;
            idle = if quiet { idle + 1 } else { 0 };
            if idle == IDLE_TICKS {
                return;
            }
        }
        panic!(
            "the speaker is not idle within {millis} ms; notes {:#?}",
            self.notes_from(0)
        );
    }

    fn notes_from(&self, from: usize) -> Vec<&Note> {
        self.ticks[from..]
            .iter()
            .flat_map(|tick| &tick.notes)
            .collect()
    }
}

/// When the caller speaks over the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Over {
    /// While the speaker still queues part of the answer.
    Queued,
    /// Once the speaker played all it was given of the answer, the provider streaming the rest
    /// slower than it plays.
    RanDry,
}

/// What a case observed, the ticks counted from the first tick of the talk-over.
#[derive(Debug)]
struct Outcome {
    provider: Provider,
    cut: ResponseId,
    /// The ticks of the talk-over in which playout was stopped for a barge-in.
    stopped: Vec<usize>,
    /// The ticks of the talk-over or after in which the device played the cut answer.
    cut_played: Vec<usize>,
    /// Which responses the device played from the talk-over on, in the order first played.
    played_after: Vec<usize>,
    /// The response the device played next, and the transcript of it.
    next: Option<ResponseId>,
    next_said: Vec<String>,
    /// What the user was heard to say from the talk-over on.
    heard: Vec<String>,
}

/// The caller asks, the answer starts playing, and the caller speaks over it for `talk_ms`; the
/// conversation goes on until the next answer plays.
fn talk_over(provider: Provider, talk_ms: usize, late: usize, over: Over) -> Outcome {
    let mut call = Call::open(provider, late);
    call.speak(100);
    call.pause_until(1_000, "the answer plays", |call| call.played(1));
    match over {
        Over::Queued => assert!(
            call.conversation.speaker().queued() > 0,
            "{provider:?}: the answer is still playing when the talk-over starts"
        ),
        Over::RanDry => call.pause_until(1_000, "the speaker plays what it has", |call| {
            call.conversation.speaker().queued() == 0
        }),
    }
    let cut = call.conversation.session().response(1);
    let started = call.now();
    call.speak(talk_ms);
    call.pause_until(2_000, "the next answer plays", |call| call.played(2));
    call.play_out(10_000);

    let after = &call.ticks[started..];
    let stopped = after
        .iter()
        .enumerate()
        .filter(|(_, tick)| {
            tick.notes
                .iter()
                .any(|note| matches!(note, Note::BargeIn { .. }))
        })
        .map(|(at, _)| at)
        .collect();
    let cut_played = after
        .iter()
        .enumerate()
        .filter(|(_, tick)| tick.played.contains(&1))
        .map(|(at, _)| at)
        .collect();
    let mut played_after = Vec::new();
    for n in after.iter().flat_map(|tick| &tick.played) {
        if !played_after.contains(n) {
            played_after.push(*n);
        }
    }
    let next = played_after
        .iter()
        .find(|n| **n != 1)
        .map(|n| call.conversation.session().response(*n));
    let notes = call.notes_from(started);
    let next_said = notes
        .iter()
        .filter_map(|note| match note {
            Note::Said { response_id, text } if Some(*response_id) == next => Some(text.clone()),
            _ => None,
        })
        .collect();
    let heard = notes
        .iter()
        .filter_map(|note| match note {
            Note::Heard(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    Outcome {
        provider,
        cut,
        stopped,
        cut_played,
        played_after,
        next,
        next_said,
        heard,
    }
}

impl Outcome {
    /// Playout of the cut answer stopped in the talk-over's first tick, the 20 ms frame in which
    /// the session heard the speech start.
    fn stops_at_once(&self) -> &Self {
        assert_eq!(
            self.stopped.first(),
            Some(&0),
            "{:?}: playout stops in the talk-over's first tick: {self:#?}",
            self.provider
        );
        self
    }

    /// No frame of the cut answer played from the talk-over's first tick on.
    fn never_plays_the_cut_answer_again(&self) -> &Self {
        assert!(
            self.cut_played.is_empty(),
            "{:?}: the cut answer {} played again in ticks {:?} of the talk-over: {self:#?}",
            self.provider,
            self.cut,
            self.cut_played
        );
        assert_eq!(
            self.played_after,
            [2],
            "{:?}: only the next answer plays after the talk-over started: {self:#?}",
            self.provider
        );
        self
    }

    /// The next answer played is a new response, answering what the caller said in the
    /// talk-over.
    fn answers_the_talk_over(&self) -> &Self {
        let next = self.next.expect("a next answer played");
        assert_ne!(
            next, self.cut,
            "{:?}: the next answer is a new response",
            self.provider
        );
        assert_eq!(
            self.heard,
            [TALK_OVER],
            "{:?}: the talk-over is what the user was heard to say: {self:#?}",
            self.provider
        );
        assert_eq!(
            self.next_said,
            [TALK_OVER_REPLY.replace("{heard}", TALK_OVER)],
            "{:?}: the next answer answers the talk-over: {self:#?}",
            self.provider
        );
        self
    }
}

/// The cases, one module per demo session.
macro_rules! cases {
    ($($session:ident: $provider:expr;)*) => {$(
        mod $session {
            use super::{Over, Provider, talk_over};

            const PROVIDER: Provider = $provider;

            /// Half a second of speech over the answer: playout stops at once, nothing of the
            /// answer plays again, and the interruption is answered. Today's behaviour, pinned.
            #[test]
            fn a_short_interruption_of_half_a_second_stops_the_answer_at_once_and_is_answered() {
                talk_over(PROVIDER, 500, 0, Over::Queued)
                    .stops_at_once()
                    .never_plays_the_cut_answer_again()
                    .answers_the_talk_over();
            }

            #[test]
            fn a_long_talk_over_of_6_s_never_resumes_the_cut_answer_and_the_next_answer_answers_it(
            ) {
                talk_over(PROVIDER, 6_000, 0, Over::Queued)
                    .stops_at_once()
                    .never_plays_the_cut_answer_again()
                    .answers_the_talk_over();
            }

            #[test]
            fn a_long_talk_over_of_12_s_never_resumes_the_cut_answer_and_the_next_answer_answers_it(
            ) {
                talk_over(PROVIDER, 12_000, 0, Over::Queued)
                    .stops_at_once()
                    .never_plays_the_cut_answer_again()
                    .answers_the_talk_over();
            }

            /// A "mm-hm" of 0.3 s is speech like any other to the conversation: it stops
            /// playout, the cut answer does not come back, and the model answers the
            /// backchannel as a turn. Today's behaviour, pinned.
            #[test]
            fn a_backchannel_cuts_the_answer_today_and_is_answered_as_a_turn() {
                talk_over(PROVIDER, 300, 0, Over::Queued)
                    .stops_at_once()
                    .never_plays_the_cut_answer_again()
                    .answers_the_talk_over();
            }
        }
    )*};
}

cases! {
    openai: Provider::OpenAi;
    openai_interrupting: Provider::OpenAiInterrupting;
    gpt_live: Provider::GptLive;
    nova: Provider::Nova;
}

/// The provider-side cases on the OpenAI Realtime session with `auto_interrupt` on: the server
/// goes on streaming the cut response after the speech start, and `Conversation`, which truncated
/// that response at the barge-in, drops what comes.
mod openai_interrupting_streaming_on {
    use super::{LATE_FRAMES, Over, Provider, talk_over};

    const PROVIDER: Provider = Provider::OpenAiInterrupting;

    #[test]
    fn frames_streamed_on_the_response_it_truncated_are_dropped_during_a_6_s_talk_over() {
        talk_over(PROVIDER, 6_000, LATE_FRAMES, Over::Queued)
            .stops_at_once()
            .never_plays_the_cut_answer_again()
            .answers_the_talk_over();
    }

    #[test]
    fn frames_streamed_on_the_response_it_truncated_are_dropped_during_a_12_s_talk_over() {
        talk_over(PROVIDER, 12_000, LATE_FRAMES, Over::Queued)
            .stops_at_once()
            .never_plays_the_cut_answer_again()
            .answers_the_talk_over();
    }

    /// The speaker has played all it was given of the answer when the caller starts a 6 s
    /// talk-over, so there is nothing to stop or truncate, and the deltas the server still had in
    /// flight for the response it cut arrive after: none of them plays.
    #[test]
    fn frames_in_flight_after_the_speaker_ran_dry_are_dropped_during_a_6_s_talk_over() {
        talk_over(PROVIDER, 6_000, LATE_FRAMES, Over::RanDry)
            .never_plays_the_cut_answer_again()
            .answers_the_talk_over();
    }
}

/// The provider-side cases on GPT-Live: the session goes on streaming the cut response after the
/// speech start, as the binding delivers the rest of the cut sentence on it, and `Conversation`
/// drops what comes; the next answer, a new response, plays.
mod gpt_live_streaming_on {
    use super::{LATE_FRAMES, Over, Provider, talk_over};

    const PROVIDER: Provider = Provider::GptLive;

    /// The provider streams half a second more of the cut response after the caller started a
    /// 6 s talk-over, then completes it: none of those frames plays, and the next answer answers
    /// the talk-over.
    #[test]
    fn frames_streamed_on_the_cut_answer_are_dropped_during_a_6_s_talk_over() {
        talk_over(PROVIDER, 6_000, LATE_FRAMES, Over::Queued)
            .stops_at_once()
            .never_plays_the_cut_answer_again()
            .answers_the_talk_over();
    }

    #[test]
    fn frames_streamed_on_the_cut_answer_are_dropped_during_a_12_s_talk_over() {
        talk_over(PROVIDER, 12_000, LATE_FRAMES, Over::Queued)
            .stops_at_once()
            .never_plays_the_cut_answer_again()
            .answers_the_talk_over();
    }

    /// The provider streams slower than the speaker plays: the speaker has played all it was
    /// given when the caller starts a 6 s talk-over, so there is nothing to stop, and the rest of
    /// the response arrives after: none of it plays.
    #[test]
    fn frames_of_the_cut_answer_after_the_speaker_ran_dry_are_dropped_during_the_talk_over() {
        talk_over(PROVIDER, 6_000, LATE_FRAMES, Over::RanDry)
            .never_plays_the_cut_answer_again()
            .answers_the_talk_over();
    }
}
