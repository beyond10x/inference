//! The comparison over two scripted providers on a fake clock, without network: each run's
//! figures, their medians and spreads, the providers' order by each measure, the usage of each
//! run, and the table.
//!
//! Both mocks hear speech in any input frame with a sample other than zero and answer each
//! question at scripted delays from the end of its speech. `fast` delivers its answer's audio at
//! once and needs the caller to cancel on barge-in, as OpenAI Realtime with `auto_interrupt`
//! off; `slow` delivers one 20 ms frame each 20 ms and stops speaking on its own side, as
//! GPT-Live does. The clock moves only when a session waits for an event, so every figure is
//! exact.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{AudioFrame, SampleClockPosition};
use inference_duplex::inference_core::model::{
    Adapter, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, ResponseUsage, SessionConfig, SessionError,
    SessionEvent, SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle,
    SpeechSessionId, TokenCounts, TuningSupport, TurnDetection, TurnTuning,
};
use realtime_voice::compare::{
    Billing, Clip, Comparison, Measure, Measured, Outcome, Plan, Spread, run,
};
use realtime_voice::{Counts, Reported, pcm16};

const SPEECH: &[u8] =
    include_bytes!("../../../adapters/inference-openai/tests/fixtures/speech.wav");

/// Samples of one 20 ms frame at 24 kHz.
const FRAME: u64 = 480;

const USAGE: ResponseUsage = ResponseUsage {
    input: TokenCounts {
        text_tokens: 10,
        audio_tokens: 20,
        cached_tokens: 0,
    },
    output: TokenCounts {
        text_tokens: 5,
        audio_tokens: 40,
        cached_tokens: 0,
    },
};

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// A clock that moves only when it is moved.
#[derive(Clone)]
struct Fake {
    base: Instant,
    offset: Rc<Cell<Duration>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            offset: Rc::new(Cell::new(Duration::ZERO)),
        }
    }

    fn offset(&self) -> Duration {
        self.offset.get()
    }

    fn now(&self) -> Instant {
        self.base + self.offset.get()
    }
}

/// How a scripted provider stops speaking at a barge-in.
#[derive(Debug, Clone, Copy)]
enum Stop {
    /// It reports speech started this long after the barge-in's first voiced frame and leaves
    /// the cancelling to the caller.
    SpeechStarted(u64),
    /// It stops sending voiced audio this long after the barge-in's first voiced frame and
    /// completes the answer.
    Model(u64),
}

/// One run of a scripted provider, in milliseconds from the end of the question's speech, each a
/// multiple of 20 ms, the pace input is sent at: off that grid a barge-in is sent at the next
/// frame and the answer's playout ends at its frame's end, and the figures move by the remainder.
#[derive(Debug, Clone, Copy)]
struct Script {
    transcript: u64,
    audio: u64,
    /// The voiced audio of the answer.
    answer: u64,
    /// All the answer's audio at `audio`, or one frame each 20 ms from it.
    burst: bool,
    /// With `burst`, the completion; otherwise the silent audio after the voiced audio before it.
    end: u64,
    stop: Stop,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    Created(ResponseId),
    Transcript(ResponseId),
    Audio(ResponseId, u64, bool),
    Usage(ResponseId),
    Completed(ResponseId),
    SpeechStarted,
}

impl Action {
    fn of(&self) -> Option<ResponseId> {
        match *self {
            Self::Created(id)
            | Self::Transcript(id)
            | Self::Audio(id, _, _)
            | Self::Usage(id)
            | Self::Completed(id) => Some(id),
            Self::SpeechStarted => None,
        }
    }
}

struct Scripted {
    adapter: Adapter,
    capabilities: SpeechCapabilities,
    runs: VecDeque<Script>,
    clock: Fake,
}

impl Scripted {
    fn new(caller_cancels: bool, runs: Vec<Script>, clock: &Fake) -> Self {
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "scripted".to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: Vec::new(),
            max_concurrency: 1,
        })
        .expect("a concurrency of one");
        let capabilities = SpeechCapabilities {
            input_formats: vec![pcm16(24_000)],
            output_formats: vec![pcm16(24_000)],
            turn_detection: vec![if caller_cancels {
                TurnDetection::ServerVad
            } else {
                TurnDetection::SemanticVad
            }],
            turn_tuning: TuningSupport::NONE,
            manual_commit: false,
            manual_response: false,
            manual_interrupt: caller_cancels,
            client_cancel: caller_cancels,
            truncation: false,
            tool_calling: false,
        };
        Self {
            adapter,
            capabilities,
            runs: runs.into(),
            clock: clock.clone(),
        }
    }

    fn config(&self) -> SessionConfig {
        SessionConfig {
            voice: "test".to_owned(),
            instructions: String::new(),
            input_format: pcm16(24_000),
            output_format: pcm16(24_000),
            turn_detection: self.capabilities.turn_detection[0],
            turn_tuning: TurnTuning::default(),
            auto_response: true,
            auto_interrupt: !self.capabilities.client_cancel,
            tools: Vec::new(),
            idle_limit: Duration::from_secs(30),
        }
    }
}

impl NativeSpeechModel for Scripted {
    type Session = ScriptedSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SessionConfig) -> Result<ScriptedSession, OpenError> {
        let script = self.runs.pop_front().expect("a script for each run");
        let (ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config,
            self.clock.now(),
        )?;
        Ok(ScriptedSession {
            ledger,
            clock: self.clock.clone(),
            script,
            pending: Vec::new(),
            speaking: false,
            speech_starts: 0,
            last_voiced: Duration::ZERO,
            answer: None,
        })
    }
}

struct ScriptedSession {
    ledger: SessionLedger,
    clock: Fake,
    script: Script,
    pending: Vec<(Duration, Action)>,
    speaking: bool,
    speech_starts: u32,
    last_voiced: Duration,
    answer: Option<ResponseId>,
}

impl ScriptedSession {
    /// The answer to speech that ended at `end`.
    fn answer(&mut self, end: Duration) {
        let script = self.script;
        let id = ResponseId::generate();
        self.answer = Some(id);
        let at = |millis: u64| end + ms(millis);
        self.pending
            .push((at(script.transcript), Action::Created(id)));
        self.pending
            .push((at(script.transcript), Action::Transcript(id)));
        let voiced = script.answer / 20;
        for index in 0..voiced {
            let when = if script.burst {
                at(script.audio)
            } else {
                at(script.audio + 20 * index)
            };
            self.pending.push((when, Action::Audio(id, index, true)));
        }
        if script.burst {
            self.pending.push((at(script.end), Action::Usage(id)));
            self.pending.push((at(script.end), Action::Completed(id)));
        } else {
            let silent = script.end / 20;
            for index in voiced..voiced + silent {
                self.pending.push((
                    at(script.audio + 20 * index),
                    Action::Audio(id, index, false),
                ));
            }
            let done = at(script.audio + 20 * (voiced + silent));
            self.pending.push((done, Action::Usage(id)));
            self.pending.push((done, Action::Completed(id)));
        }
    }

    /// The barge-in's first voiced frame arrived at `at`.
    fn barge_in(&mut self, at: Duration) {
        let id = self.answer.expect("an answer playing");
        match self.script.stop {
            Stop::SpeechStarted(after) => {
                self.pending.push((at + ms(after), Action::SpeechStarted))
            }
            Stop::Model(after) => {
                let cut = at + ms(after);
                self.pending
                    .retain(|(when, action)| action.of() != Some(id) || *when < cut);
                self.pending.push((cut, Action::Usage(id)));
                self.pending.push((cut, Action::Completed(id)));
            }
        }
    }

    fn apply(&mut self, action: Action) {
        let now = self.clock.now();
        match action {
            Action::Created(id) => self.ledger.response_created(id, now),
            Action::Transcript(id) => self.ledger.output_transcript(id, "Paris".to_owned(), now),
            Action::Audio(id, index, voiced) => {
                let sample: i16 = if voiced { 8_192 } else { 0 };
                let frame = AudioFrame {
                    format: pcm16(24_000),
                    position: SampleClockPosition::new(index * FRAME).expect("small"),
                    sample_count: FRAME as u32,
                    data: sample.to_le_bytes().repeat(FRAME as usize),
                };
                self.ledger.output_audio(id, frame, now)
            }
            Action::Usage(id) => self.ledger.response_usage(id, USAGE, now),
            Action::Completed(id) => self.ledger.response_completed(id, now),
            Action::SpeechStarted => self
                .ledger
                .speech_started(SampleClockPosition::new(0).expect("zero"), now),
        }
        .expect("the script follows the contract");
    }
}

impl SpeechSessionHandle for ScriptedSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        let voiced = frame.data.iter().any(|&byte| byte != 0);
        self.ledger.accept_input(frame)?;
        let now = self.clock.offset();
        if voiced {
            if !self.speaking {
                self.speaking = true;
                self.speech_starts += 1;
                if self.speech_starts == 3 {
                    self.barge_in(now);
                }
            }
            self.last_voiced = now;
        } else if self.speaking {
            self.speaking = false;
            if self.speech_starts < 3 {
                self.answer(self.last_voiced);
            }
        }
        Ok(())
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.ledger.commit_input(self.clock.now())
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.ledger.request_response(self.clock.now())
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.ledger.cancel(response_id)?;
        self.pending
            .retain(|(_, action)| action.of() != Some(response_id));
        self.ledger
            .response_usage(response_id, USAGE, self.clock.now())
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
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

    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent> {
        let deadline = self.clock.offset() + wait;
        loop {
            if let Some(event) = self.ledger.next_event() {
                return Some(event);
            }
            if self.ledger.session().require_open().is_err() {
                return None;
            }
            let due = self
                .pending
                .iter()
                .enumerate()
                .min_by_key(|(_, (when, _))| *when)
                .map(|(index, (when, _))| (index, *when));
            match due {
                Some((index, when)) if when <= deadline => {
                    self.clock.offset.set(when.max(self.clock.offset()));
                    let (_, action) = self.pending.remove(index);
                    self.apply(action);
                }
                _ => {
                    self.clock.offset.set(deadline);
                    return None;
                }
            }
        }
    }
}

/// 200 ms of speech, then 100 ms of silence, at 24 kHz.
fn utterance() -> Clip {
    let mut samples = vec![0.25_f32; 4_800];
    samples.extend([0.0; 2_400]);
    Clip::new(
        samples,
        std::num::NonZeroU32::new(24_000).expect("non-zero"),
    )
}

fn plan() -> Plan {
    let mut plan = Plan::new(utterance());
    plan.barge_in = Some(utterance());
    plan
}

fn fast(transcript: u64, audio: u64, answer: u64) -> Script {
    Script {
        transcript,
        audio,
        answer,
        burst: true,
        end: 900,
        stop: Stop::SpeechStarted(200),
    }
}

fn slow(transcript: u64, audio: u64) -> Script {
    Script {
        transcript,
        audio,
        answer: 1_000,
        burst: false,
        end: 200,
        stop: Stop::Model(100),
    }
}

fn runs(caller_cancels: bool, scripts: Vec<Script>, seconds: bool) -> Vec<Measured> {
    runs_with(&plan(), caller_cancels, scripts, seconds)
}

fn runs_with(
    plan: &Plan,
    caller_cancels: bool,
    scripts: Vec<Script>,
    seconds: bool,
) -> Vec<Measured> {
    let clock = Fake::new();
    let count = scripts.len();
    let mut model = Scripted::new(caller_cancels, scripts, &clock);
    let now = {
        let clock = clock.clone();
        move || clock.now()
    };
    let billing = if seconds {
        Billing {
            reported: Reported::OPENAI_LIVE,
            seconds: |_: &ScriptedSession| Some(12.5),
        }
    } else {
        Billing::tokens(Reported::OPENAI)
    };
    (0..count)
        .map(|_| {
            let config = model.config();
            run(&mut model, config, plan, &now, &billing).expect("the session opens")
        })
        .collect()
}

fn spread(median: u64, lowest: u64, highest: u64, measured: usize, runs: usize) -> Option<Spread> {
    Some(Spread {
        median: ms(median),
        lowest: ms(lowest),
        highest: ms(highest),
        measured,
        runs,
    })
}

#[test]
fn two_scripted_providers_give_their_figures_their_order_and_the_table() {
    let fast_runs = runs(
        true,
        vec![
            fast(250, 300, 1_500),
            fast(270, 340, 1_500),
            fast(260, 320, 1_500),
        ],
        false,
    );
    let slow_runs = runs(
        false,
        vec![slow(500, 600), slow(550, 700), slow(520, 640)],
        true,
    );
    for run in fast_runs.iter().chain(&slow_runs) {
        assert_eq!(run.notes, Vec::<String>::new(), "{run:?}");
    }
    assert_eq!(fast_runs[0].first_audio, Some(ms(300)));
    assert_eq!(fast_runs[0].first_transcript, Some(ms(250)));
    assert_eq!(fast_runs[0].response_end, Some(ms(900)));
    assert_eq!(fast_runs[0].barge_in_stop, Some(ms(200)));
    assert_eq!(slow_runs[0].response_end, Some(ms(1_800)));
    assert_eq!(slow_runs[0].barge_in_stop, Some(ms(100)));

    let mut comparison = Comparison::new();
    comparison.ran("fast", fast_runs);
    comparison.ran("slow", slow_runs);
    comparison.skipped("nova", "blocked on AWS access");

    assert_eq!(
        comparison.spread("fast", Measure::FirstAudio),
        spread(320, 300, 340, 3, 3)
    );
    assert_eq!(
        comparison.spread("fast", Measure::FirstTranscript),
        spread(260, 250, 270, 3, 3)
    );
    assert_eq!(
        comparison.spread("fast", Measure::ResponseEnd),
        spread(900, 900, 900, 3, 3)
    );
    assert_eq!(
        comparison.spread("fast", Measure::BargeInStop),
        spread(200, 200, 200, 3, 3)
    );
    assert_eq!(
        comparison.spread("slow", Measure::FirstAudio),
        spread(640, 600, 700, 3, 3)
    );
    assert_eq!(
        comparison.spread("slow", Measure::FirstTranscript),
        spread(520, 500, 550, 3, 3)
    );
    assert_eq!(
        comparison.spread("slow", Measure::ResponseEnd),
        spread(1_840, 1_800, 1_900, 3, 3)
    );
    assert_eq!(
        comparison.spread("slow", Measure::BargeInStop),
        spread(100, 100, 100, 3, 3)
    );
    assert_eq!(comparison.spread("nova", Measure::FirstAudio), None);

    assert_eq!(comparison.ranking(Measure::FirstAudio), ["fast", "slow"]);
    assert_eq!(comparison.ranking(Measure::ResponseEnd), ["fast", "slow"]);
    assert_eq!(comparison.ranking(Measure::BargeInStop), ["slow", "fast"]);

    let Some(Outcome::Ran(fast_runs)) = comparison.outcome("fast") else {
        panic!("fast ran");
    };
    let Some(Outcome::Ran(slow_runs)) = comparison.outcome("slow") else {
        panic!("slow ran");
    };
    for run in fast_runs {
        assert_eq!(run.usage.responses, 2);
        assert_eq!(run.usage.unreported, 0);
        assert_eq!(
            run.usage.counts,
            Counts {
                input_text: Some(20),
                input_audio: Some(40),
                input_cached: Some(0),
                output_text: Some(10),
                output_audio: Some(80),
            }
        );
        assert_eq!(run.seconds, None);
    }
    for run in slow_runs {
        assert_eq!(run.usage.responses, 2);
        assert_eq!(
            run.usage.counts,
            Counts {
                input_text: Some(20),
                input_audio: None,
                input_cached: Some(0),
                output_text: Some(10),
                output_audio: None,
            }
        );
        assert_eq!(run.seconds, Some(12.5));
    }

    let table = comparison.to_string();
    println!("{table}");
    let row = |name: &str| {
        table
            .lines()
            .find(|line| line.starts_with(&format!("{name} ")))
            .unwrap_or_else(|| panic!("a row for {name} in\n{table}"))
            .to_owned()
    };
    let fast_row = row("fast");
    for cell in [
        "320 [300-340] 3/3",
        "260 [250-270] 3/3",
        "900 [900-900] 3/3",
        "200 [200-200] 3/3",
    ] {
        assert!(fast_row.contains(cell), "{cell} in {fast_row}");
    }
    assert!(row("slow").contains("1840 [1800-1900] 3/3"), "{table}");
    assert!(
        row("nova").contains("skipped: blocked on AWS access"),
        "{table}"
    );
    assert!(table.contains("billed 12.5 s"), "{table}");
}

#[test]
fn an_answer_that_finished_playing_before_the_barge_in_is_not_measured_and_says_why() {
    let measured = runs(true, vec![fast(250, 300, 300)], false);
    assert_eq!(measured[0].first_audio, Some(ms(300)));
    assert_eq!(measured[0].barge_in_stop, None);
    assert!(
        measured[0]
            .notes
            .iter()
            .any(|note| note.contains("finished playing before the barge-in")
                && note.contains("ended 300 ms after its first audio")
                && note.contains("started 500 ms after it")),
        "{:?}",
        measured[0].notes
    );
}

#[test]
fn the_barge_in_speech_starts_barge_after_into_the_answer_whatever_silence_leads_its_clip() {
    let mut led = vec![0.0_f32; 9_600];
    led.extend([0.25; 4_800]);
    let mut plan = plan();
    plan.barge_in = Some(Clip::new(
        led,
        std::num::NonZeroU32::new(24_000).expect("non-zero"),
    ));
    let measured = runs_with(&plan, true, vec![fast(250, 300, 600)], false);
    assert_eq!(
        measured[0].barge_in_stop,
        Some(ms(100)),
        "speech 500 ms into an answer of 600 ms: {:?}",
        measured[0].notes
    );
}

#[test]
fn the_spread_of_an_even_count_takes_the_mean_of_the_middle_two_and_counts_the_unmeasured() {
    let spread = Spread::of(&[Some(ms(300)), None, Some(ms(100))]).expect("two measured");
    assert_eq!(spread.median, ms(200));
    assert_eq!((spread.lowest, spread.highest), (ms(100), ms(300)));
    assert_eq!((spread.measured, spread.runs), (2, 3));
    assert_eq!(spread.to_string(), "200 [100-300] 2/3");
    assert_eq!(Spread::of(&[None, None]), None);
}

#[test]
fn the_question_fixture_reads_as_a_clip_of_its_length() {
    let clip = Clip::from_wav(SPEECH).expect("16-bit PCM");
    let length = clip.duration();
    assert!(
        length > ms(2_150) && length < ms(2_160),
        "47523 samples at 22050 Hz, not {length:?}"
    );
    assert!(Clip::from_wav(b"not a wav file").is_err());
}
