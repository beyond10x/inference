//! One latency report over both topologies, on a fake clock, without network: a native speech
//! model, the testkit's `MockSpeechModel`, and a cascade of the testkit's recogniser, generator
//! and synthesiser, each step delayed by a known amount.
//!
//! None of the mocks has a delay hook, so each is wrapped here: the wrapper moves the clock the
//! arm reads forward by the step's scripted delay before it hands the step's first event over.
//! The clock moves only then and when the arm waits, so no wall-clock sleep decides a figure.
//!
//! Tolerance: one input frame, 20 ms, the pace both arms send at. A figure taken from the end of
//! speech lies within one frame of its scripted value, since each arm hears the end of speech at
//! a frame boundary; a step taken between two instants of the arm is exact.

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{AudioFrame, SampleClockPosition};
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle, TurnDetection,
    TurnTuning,
};
use inference_elevenlabs::{
    CredentialError, CredentialFuture, CredentialSource, Endpoint, OutputFormat,
};
use inference_llm::Generator;
use inference_llm::llm_core::{
    self, BoxFuture, Cancel, Capabilities, Provenance, StreamSink, TurnOutcome, TurnRequest,
};
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::model::{Adapter, ModelId};
use inference_stt::inference_text::{ReviseSpan, SegmentId};
use inference_stt::{
    ConcurrencyLimitReached as SttLimit, InputEnded, Recogniser, RecognitionHandle,
    RecognitionRequest, RevisionRefused, TranscriptEvent, TranscriptOutput,
};
use inference_testkit::duplex::mock::{MockSpeechModel, MockSpeechSession, Reply, ScriptedTurn};
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stt::{Cue, MockConfiguration as SttConfiguration, MockRecogniser};
use inference_testkit::stt::{MockRecognition, ScriptStep};
use inference_testkit::tts::MockSynthesiser;
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesis};
use inference_tts::inference_text::{AudioPlayed, OutputSegment, SegmentDiscarded, SegmentQueued};
use inference_tts::{
    ConcurrencyLimitReached as TtsLimit, DiscardRefused, OfferRefused, PlaybackReceipt,
    PlaybackRefused, SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};
use realtime_voice::compare::{
    Billing, Cascade, Clip, Comparison, Measure, Measured, NO_HOSTED_TTS, Plan, Playout, Topology,
    hosted_tts, run, run_cascade,
};
use realtime_voice::{Reported, pcm16};

/// One input frame, the pace of sending and the tolerance of a figure from the end of speech.
const FRAME: Duration = Duration::from_millis(20);

/// The native model's scripted delays, each from the step before it: the end of speech heard,
/// its first transcript, its first audio, its completion.
const NATIVE_TRANSCRIPT: Duration = Duration::from_millis(250);
const NATIVE_AUDIO: Duration = Duration::from_millis(50);
const NATIVE_END: Duration = Duration::from_millis(400);

/// The cascade's endpoint: the silence after speech that ends the recogniser's input.
const HOLD: Duration = Duration::from_millis(200);
/// The cascade's scripted delays per step: the recogniser's final transcript after the end of
/// its input, the LLM's first token after the transcript, and the TTS's first audio after the
/// token.
const STT_DELAY: Duration = Duration::from_millis(120);
const LLM_DELAY: Duration = Duration::from_millis(200);
const TTS_DELAY: Duration = Duration::from_millis(90);
/// The wall-clock wait of the test synthesiser between a segment's offer and its first frame,
/// which the synthesis's own time to first audio measures. Well under `SIMULATED_STALL`, so the
/// simulated clock does not move meanwhile.
const TTS_OFFER_DELAY: Duration = Duration::from_millis(30);
/// The cascade's detector: voiced input this long opens a turn, and so stops the playout.
const ATTACK: Duration = Duration::from_millis(30);
/// The time the playout takes to stop.
const STOP_DELAY: Duration = Duration::from_millis(10);

/// The wall-clock time a case may take: every case runs on a fake clock and takes milliseconds,
/// so an arm that spins or hangs fails the case instead of holding the test binary.
const DEADLINE: Duration = Duration::from_secs(20);

/// Runs `case` on a thread of its own, failing once `DEADLINE` passed; a panic of the case
/// fails it as well, its message printed by the case's thread.
fn within_deadline(case: fn()) {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        case();
        let _ = sender.send(());
    });
    match receiver.recv_timeout(DEADLINE) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("the case did not end within {DEADLINE:?} of wall-clock time: it hangs")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the case failed; see its panic above"),
    }
}

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// A clock that moves only when it is moved, shared across threads.
#[derive(Debug, Clone)]
struct Fake {
    base: Instant,
    offset: Arc<Mutex<Duration>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            offset: Arc::new(Mutex::new(Duration::ZERO)),
        }
    }

    fn now(&self) -> Instant {
        self.base + *self.offset.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn advance(&self, by: Duration) {
        *self.offset.lock().unwrap_or_else(PoisonError::into_inner) += by;
    }
}

// The native arm: the testkit's model, its events delayed on the fake clock.

struct DelayedSpeech {
    inner: MockSpeechModel,
    clock: Fake,
}

impl NativeSpeechModel for DelayedSpeech {
    type Session = DelayedSession;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        self.inner.capabilities()
    }

    fn open(&mut self, config: SessionConfig) -> Result<DelayedSession, OpenError> {
        Ok(DelayedSession {
            inner: self.inner.open(config)?,
            clock: self.clock.clone(),
            transcript: false,
            audio: false,
        })
    }
}

struct DelayedSession {
    inner: MockSpeechSession,
    clock: Fake,
    transcript: bool,
    audio: bool,
}

impl SpeechSessionHandle for DelayedSession {
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
        let Some(event) = self.inner.next_event(wait) else {
            self.clock.advance(wait);
            return None;
        };
        match &event {
            SessionEvent::OutputTranscriptDelta(_) if !self.transcript => {
                self.transcript = true;
                self.clock.advance(NATIVE_TRANSCRIPT);
            }
            SessionEvent::OutputAudio(_) if !self.audio => {
                self.audio = true;
                self.clock.advance(NATIVE_AUDIO);
            }
            SessionEvent::ResponseCompleted(_) => self.clock.advance(NATIVE_END),
            _ => {}
        }
        Some(event)
    }
}

fn native_config() -> SessionConfig {
    SessionConfig {
        voice: "test".to_owned(),
        instructions: String::new(),
        input_format: pcm16(24_000),
        output_format: pcm16(24_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(30),
    }
}

fn native_run(plan: &Plan) -> Measured {
    let clock = Fake::new();
    let mut model = DelayedSpeech {
        inner: MockSpeechModel::native(vec![ScriptedTurn {
            heard: "what is the capital of france".to_owned(),
            reply: Reply::Say("Paris".to_owned()),
        }]),
        clock: clock.clone(),
    };
    let now = move || clock.now();
    run(
        &mut model,
        native_config(),
        plan,
        &now,
        &Billing::tokens(Reported::OPENAI),
    )
    .expect("the mock session opens")
}

// The cascade arm: the testkit's recogniser, generator and synthesiser, each delayed.

#[derive(Clone)]
struct DelayedRecogniser {
    inner: MockRecogniser,
    clock: Fake,
}

impl Recogniser for DelayedRecogniser {
    type Handle = DelayedRecognition;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(&mut self, request: RecognitionRequest) -> Result<DelayedRecognition, SttLimit> {
        Ok(DelayedRecognition {
            inner: self.inner.start(request)?,
            clock: self.clock.clone(),
            committed: false,
        })
    }
}

struct DelayedRecognition {
    inner: MockRecognition,
    clock: Fake,
    committed: bool,
}

impl Iterator for DelayedRecognition {
    type Item = EventEnvelope<TranscriptEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        let event = self.inner.next()?;
        if matches!(event.payload, TranscriptEvent::Committed(_)) && !self.committed {
            self.committed = true;
            self.clock.advance(STT_DELAY);
        }
        Some(event)
    }
}

impl RecognitionHandle for DelayedRecognition {
    fn output(&self) -> &TranscriptOutput {
        self.inner.output()
    }

    fn offer_frame(&mut self, frame: AudioFrame) -> Result<(), InputEnded> {
        self.inner.offer_frame(frame)
    }

    fn end_input(&mut self) -> Result<(), InputEnded> {
        self.inner.end_input()
    }

    fn revise(&mut self, input: ReviseSpan) -> Result<(), RevisionRefused> {
        self.inner.revise(input)
    }
}

/// The mock generator's model, each turn starting `LLM_DELAY` late on the fake clock.
#[derive(Debug)]
struct DelayedModel {
    inner: Generator,
    clock: Fake,
}

impl llm_core::Model for DelayedModel {
    fn provenance(&self) -> &Provenance {
        self.inner.model().provenance()
    }

    fn capabilities(&self) -> &Capabilities {
        self.inner.model().capabilities()
    }

    fn turn<'a>(
        &'a self,
        request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, llm_core::Error>> {
        self.clock.advance(LLM_DELAY);
        self.inner.model().turn(request, sink, cancel)
    }
}

struct DelayedSynthesiser {
    inner: MockSynthesiser,
    clock: Fake,
    /// The segments that sent a frame, across every synthesis.
    spoken: Arc<AtomicUsize>,
}

impl Synthesiser for DelayedSynthesiser {
    type Handle = DelayedSynthesis;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(&mut self) -> Result<DelayedSynthesis, TtsLimit> {
        Ok(DelayedSynthesis {
            inner: self.inner.start()?,
            clock: self.clock.clone(),
            audio: false,
            heard: HashSet::new(),
            spoken: Arc::clone(&self.spoken),
        })
    }
}

struct DelayedSynthesis {
    inner: MockSynthesis,
    clock: Fake,
    audio: bool,
    heard: HashSet<SegmentId>,
    spoken: Arc<AtomicUsize>,
}

impl Iterator for DelayedSynthesis {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        let event = self.inner.next()?;
        if let SynthesisEvent::Audio(audio) = &event
            && self.heard.insert(audio.segment_id())
        {
            self.spoken.fetch_add(1, Ordering::SeqCst);
        }
        if matches!(event, SynthesisEvent::Audio(_)) && !self.audio {
            self.audio = true;
            self.clock.advance(TTS_DELAY);
        }
        Some(event)
    }
}

impl SynthesisHandle for DelayedSynthesis {
    fn output(&self) -> &SynthesisOutput {
        self.inner.output()
    }

    /// Offers the segment and waits [`TTS_OFFER_DELAY`] on the wall clock, so its first frame
    /// comes at least that long after its offer.
    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        let queued = self.inner.offer_segment(segment)?;
        std::thread::sleep(TTS_OFFER_DELAY);
        Ok(queued)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.inner.end_utterance()
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.inner.acknowledge_playback(segment_id, receipt)
    }

    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        self.inner.discard(segment_id)
    }
}

/// A speaker whose playout stops in `STOP_DELAY`, on the fake clock.
struct DelayedPlayout {
    clock: Fake,
    stops: usize,
}

impl Playout for DelayedPlayout {
    fn play(&mut self, _frame: &AudioFrame) {}

    fn stop(&mut self) {
        self.stops += 1;
        self.clock.advance(STOP_DELAY);
    }
}

const ANSWER: [&str; 4] = [
    "Paris",
    " is the capital",
    " of France and has been",
    " for ages.",
];

fn cascade_run(plan: &Plan) -> (Measured, usize) {
    cascade_answering(plan, &ANSWER)
}

/// A run of the cascade whose LLM answers with the deltas `answer`; the mock synthesiser speaks
/// one 10 ms frame per word.
fn cascade_answering(plan: &Plan, answer: &[&str]) -> (Measured, usize) {
    let (measured, stops, _) = cascade_spoken(plan, answer);
    (measured, stops)
}

/// [`cascade_answering`], with the number of segments the synthesiser sent a frame of.
fn cascade_spoken(plan: &Plan, answer: &[&str]) -> (Measured, usize, usize) {
    let clock = Fake::new();
    let script = vec![
        Cue {
            after_frames: 1,
            step: ScriptStep::Hypothesis("what is the capital of france".to_owned()),
        },
        Cue {
            after_frames: usize::MAX,
            step: ScriptStep::Commit { span: 0 },
        },
    ];
    let recogniser = DelayedRecogniser {
        inner: MockRecogniser::new(SttConfiguration::Incremental, ModelId::generate(), script),
        clock: clock.clone(),
    };
    let mock: Generator = MockGenerator::new(
        LlmConfiguration::StreamingOnly,
        ModelId::generate(),
        answer.iter().copied(),
    )
    .into();
    let generator = Generator::new(
        mock.adapter().clone(),
        Arc::new(DelayedModel {
            inner: mock,
            clock: clock.clone(),
        }),
    )
    .expect("the wrapper declares what the mock declares");
    let synthesiser = DelayedSynthesiser {
        inner: MockSynthesiser::new(
            TtsConfiguration::IncrementalStreaming,
            ModelId::generate(),
            pcm16(16_000),
        ),
        clock: clock.clone(),
        spoken: Arc::new(AtomicUsize::new(0)),
    };
    let spoken = Arc::clone(&synthesiser.spoken);
    let mut cascade = Cascade::new(recogniser, generator, synthesiser, pcm16(16_000));
    cascade.hold = HOLD;
    cascade.attack = ATTACK;
    let mut playout = DelayedPlayout {
        clock: clock.clone(),
        stops: 0,
    };
    let now = {
        let clock = clock.clone();
        move || clock.now()
    };
    let wait = move |by: Duration| clock.advance(by);
    let measured = run_cascade(&mut cascade, plan, &now, &wait, &mut playout);
    (measured, playout.stops, spoken.load(Ordering::SeqCst))
}

/// 200 ms of speech, then 100 ms of silence, at 24 kHz.
fn utterance() -> Clip {
    let mut samples = vec![0.25_f32; 4_800];
    samples.extend([0.0; 2_400]);
    Clip::new(samples, NonZeroU32::new(24_000).expect("non-zero"))
}

/// That `measured` lies within one frame at or after `scripted`.
fn near(what: &str, measured: Option<Duration>, scripted: Duration) {
    let measured = measured.unwrap_or_else(|| panic!("{what} was measured"));
    assert!(
        measured >= scripted && measured <= scripted + FRAME,
        "{what}: {measured:?}, scripted {scripted:?}, tolerance {FRAME:?}"
    );
}

/// That `later - earlier` is `scripted`, exactly: both are instants of the arm.
fn step(what: &str, earlier: Option<Duration>, later: Option<Duration>, scripted: Duration) {
    let (Some(earlier), Some(later)) = (earlier, later) else {
        panic!("{what}: both ends measured, {earlier:?} and {later:?}");
    };
    assert_eq!(later.saturating_sub(earlier), scripted, "{what}");
}

fn row(table: &str, name: &str) -> String {
    table
        .lines()
        .find(|line| line.starts_with(&format!("{name} ")))
        .unwrap_or_else(|| panic!("a row for {name} in\n{table}"))
        .to_owned()
}

#[test]
fn a_native_and_a_cascade_arm_land_in_one_report_each_step_at_its_scripted_delay() {
    within_deadline(
        a_native_and_a_cascade_arm_land_in_one_report_each_step_at_its_scripted_delay_case,
    );
}

fn a_native_and_a_cascade_arm_land_in_one_report_each_step_at_its_scripted_delay_case() {
    let plan = Plan::new(utterance());
    let native = native_run(&plan);
    let (cascade, _, spoken) = cascade_spoken(&plan, &ANSWER);
    assert_eq!(native.notes, Vec::<String>::new(), "{native:?}");
    assert_eq!(cascade.notes, Vec::<String>::new(), "{cascade:?}");
    assert_eq!(native.topology, Topology::Native);
    assert_eq!(cascade.topology, Topology::Cascade);

    println!("native: {native:?}");
    println!("cascade: {cascade:?}");
    near(
        "native first transcript",
        native.first_transcript,
        NATIVE_TRANSCRIPT,
    );
    near(
        "native first audio",
        native.first_audio,
        NATIVE_TRANSCRIPT + NATIVE_AUDIO,
    );
    near(
        "native response end",
        native.response_end,
        NATIVE_TRANSCRIPT + NATIVE_AUDIO + NATIVE_END,
    );
    for measure in [
        Measure::FinalTranscript,
        Measure::FirstToken,
        Measure::TtsFirstAudio,
        Measure::TtsOfferToAudio,
    ] {
        assert_eq!(measure.of(&native), None, "{measure:?} of the native arm");
    }

    let transcript = HOLD + STT_DELAY;
    near(
        "cascade final transcript",
        cascade.final_transcript,
        transcript,
    );
    near(
        "cascade first token",
        cascade.first_token,
        transcript + LLM_DELAY,
    );
    near(
        "cascade TTS first audio",
        cascade.tts_first_audio,
        transcript + LLM_DELAY + TTS_DELAY,
    );
    let audio = transcript + LLM_DELAY + TTS_DELAY;
    near("cascade first audio", cascade.first_audio, audio);
    near("cascade response end", cascade.response_end, audio);
    assert_eq!(
        cascade.first_audio, cascade.tts_first_audio,
        "the client plays the first frame at its receipt: no answer plays before it"
    );
    step(
        "the LLM's first token after the final transcript",
        cascade.final_transcript,
        cascade.first_token,
        LLM_DELAY,
    );
    step(
        "the TTS's first audio after the first token",
        cascade.first_token,
        cascade.tts_first_audio,
        TTS_DELAY,
    );
    assert_eq!(cascade.first_transcript, None);
    assert!(spoken > 0, "the synthesiser spoke a segment");
    assert_eq!(
        cascade.tts_segment_first_audio.len(),
        spoken,
        "story:elevenlabs-tts-provider-cancel: the synthesis's time to first audio of each spoken \
         segment reaches the report: {cascade:?}"
    );
    for first_audio in &cascade.tts_segment_first_audio {
        assert!(
            *first_audio >= TTS_OFFER_DELAY,
            "time to first audio {first_audio:?}, the synthesiser waits {TTS_OFFER_DELAY:?}"
        );
    }
    assert_eq!(
        Measure::TtsOfferToAudio.of(&cascade),
        cascade.tts_segment_first_audio.first().copied()
    );
    assert_eq!(native.tts_segment_first_audio, Vec::<Duration>::new());

    let mut comparison = Comparison::new();
    comparison.ran("native-mock", vec![native]);
    comparison.ran("cascade-mock", vec![cascade]);
    assert_eq!(comparison.topology("native-mock"), Some(Topology::Native));
    assert_eq!(comparison.topology("cascade-mock"), Some(Topology::Cascade));
    assert!(
        comparison
            .spread("cascade-mock", Measure::FirstToken)
            .is_some()
    );
    assert_eq!(comparison.spread("native-mock", Measure::FirstToken), None);
    assert_eq!(comparison.ranking(Measure::FirstAudio).len(), 2);

    let table = comparison.to_string();
    println!("{table}");
    let heading = table
        .lines()
        .find(|line| line.starts_with("provider"))
        .unwrap_or_else(|| panic!("a heading in\n{table}"));
    assert!(heading.contains("topology"), "{heading}");
    for measure in Measure::ALL {
        assert!(
            heading.contains(measure.name()),
            "{} in {heading}",
            measure.name()
        );
    }
    for name in [
        "first audio",
        "first transcript",
        "response end",
        "barge-in to stop",
        "final transcript",
        "first token",
        "tts first audio",
        "tts offer to audio",
    ] {
        assert!(heading.contains(name), "{name} in {heading}");
    }
    let native_row = row(&table, "native-mock");
    let cascade_row = row(&table, "cascade-mock");
    assert!(native_row.contains(" native "), "{native_row}");
    assert!(cascade_row.contains(" cascade "), "{cascade_row}");
    assert_eq!(
        native_row.matches("n/a").count(),
        4,
        "the four cascade steps: {native_row}"
    );
    assert!(!heading.contains("playout start"), "{heading}");
    assert_eq!(
        cascade_row.matches("n/a").count(),
        1,
        "the native first transcript: {cascade_row}"
    );
    assert!(!native_row.contains(" 0 [0-0]"), "{native_row}");
    let cell = |at: Option<Duration>| {
        let at = at.expect("measured").as_millis();
        format!("{at} [{at}-{at}] 1/1")
    };
    let Some(realtime_voice::compare::Outcome::Ran(runs)) = comparison.outcome("cascade-mock")
    else {
        panic!("the cascade ran");
    };
    let offer_to_audio = Measure::TtsOfferToAudio.of(&runs[0]);
    assert!(
        offer_to_audio.is_some_and(|at| at >= TTS_OFFER_DELAY),
        "tts offer to audio {offer_to_audio:?}, the synthesiser waits {TTS_OFFER_DELAY:?}"
    );
    for value in [
        runs[0].final_transcript,
        runs[0].first_token,
        runs[0].tts_first_audio,
        Measure::TtsOfferToAudio.of(&runs[0]),
    ] {
        assert!(
            cascade_row.contains(&cell(value)),
            "{} in {cascade_row}",
            cell(value)
        );
    }
}

#[test]
fn the_cascade_stops_its_playout_at_a_barge_in_after_its_detector_hears_it() {
    within_deadline(the_cascade_stops_its_playout_at_a_barge_in_after_its_detector_hears_it_case);
}

fn the_cascade_stops_its_playout_at_a_barge_in_after_its_detector_hears_it_case() {
    let mut plan = Plan::new(utterance());
    plan.barge_in = Some(utterance());
    plan.barge_after = ms(20);
    let (cascade, stops) = cascade_run(&plan);
    assert_eq!(cascade.notes, Vec::<String>::new(), "{cascade:?}");
    assert_eq!(stops, 1, "the playout stopped once");
    near(
        "cascade barge-in to stop",
        cascade.barge_in_stop,
        FRAME + STOP_DELAY,
    );
    near(
        "the latency turn's first audio, not the barge-in turn's",
        cascade.first_audio,
        HOLD + STT_DELAY + LLM_DELAY + TTS_DELAY,
    );
}

/// Ten words, 100 ms of answer on the mock synthesiser.
const SHORT_ANSWER: [&str; 4] = [
    "Paris",
    " is the capital",
    " of France and has been",
    " for.",
];

/// The barge-in's speech starts while the short answer still plays, and the answer ends before
/// the detector's attack is met: barge-in to stop runs to the end of the answer's audio as the
/// client plays it, not to a stop the detector's attack decides, and nothing is stopped.
#[test]
fn barge_in_to_stop_ends_where_the_answer_ends_before_the_detector_hears_it() {
    within_deadline(barge_in_to_stop_ends_where_the_answer_ends_before_the_detector_hears_it_case);
}

fn barge_in_to_stop_ends_where_the_answer_ends_before_the_detector_hears_it_case() {
    let mut plan = Plan::new(utterance());
    plan.barge_in = Some(utterance());
    plan.barge_after = ms(80);
    let (cascade, stops) = cascade_answering(&plan, &SHORT_ANSWER);
    println!("{cascade:?}");
    assert_eq!(cascade.notes, Vec::<String>::new(), "{cascade:?}");
    assert_eq!(
        stops, 0,
        "nothing played when the detector heard the barge-in"
    );
    let stop = cascade
        .barge_in_stop
        .expect("barge-in to stop was measured");
    assert!(
        stop < ATTACK,
        "barge-in to stop {stop:?}: the answer ended before the detector's attack of {ATTACK:?}"
    );
}

/// A source that never holds the key.
struct NoKey;

impl CredentialSource for NoKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(Err(CredentialError::Missing)))
    }
}

/// A source that holds a key that is not one.
struct FakeKey;

impl CredentialSource for FakeKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(Ok(
            inference_elevenlabs::Credential::new(b"not-a-key".to_vec()),
        )))
    }
}

#[test]
fn without_a_hosted_tts_the_cascade_arm_is_skipped_and_the_native_arm_still_reports() {
    within_deadline(
        without_a_hosted_tts_the_cascade_arm_is_skipped_and_the_native_arm_still_reports_case,
    );
}

fn without_a_hosted_tts_the_cascade_arm_is_skipped_and_the_native_arm_still_reports_case() {
    let skipped = hosted_tts(
        Endpoint::new(),
        Arc::new(NoKey),
        Some("a-voice"),
        OutputFormat::Pcm16000,
    )
    .expect_err("no key resolves");
    assert!(skipped.starts_with(NO_HOSTED_TTS), "{skipped}");
    assert_eq!(NO_HOSTED_TTS, "no hosted TTS configured");
    let no_voice = hosted_tts(
        Endpoint::new(),
        Arc::new(FakeKey),
        None,
        OutputFormat::Pcm16000,
    )
    .expect_err("no voice");
    assert!(no_voice.starts_with(NO_HOSTED_TTS), "{no_voice}");
    let configured = hosted_tts(
        Endpoint::new(),
        Arc::new(FakeKey),
        Some("a-voice"),
        OutputFormat::Pcm16000,
    );
    assert!(
        configured.is_ok(),
        "a key and a voice configure it, nothing is sent"
    );

    let plan = Plan::new(utterance());
    let mut comparison = Comparison::new();
    comparison.ran("native-mock", vec![native_run(&plan)]);
    comparison.skipped_as("cascade", Topology::Cascade, &skipped);
    assert_eq!(comparison.topology("cascade"), Some(Topology::Cascade));
    assert_eq!(comparison.spread("cascade", Measure::FirstAudio), None);
    assert!(
        comparison
            .spread("native-mock", Measure::FirstAudio)
            .is_some()
    );
    let table = comparison.to_string();
    println!("{table}");
    assert!(
        row(&table, "cascade").contains("cascade   skipped: no hosted TTS configured"),
        "{table}"
    );
    assert!(row(&table, "native-mock").contains(" native "), "{table}");
}

#[test]
fn a_cascade_measure_is_not_one_of_the_native_topology_and_the_shared_ones_are_both() {
    within_deadline(
        a_cascade_measure_is_not_one_of_the_native_topology_and_the_shared_ones_are_both_case,
    );
}

fn a_cascade_measure_is_not_one_of_the_native_topology_and_the_shared_ones_are_both_case() {
    for measure in [
        Measure::FirstAudio,
        Measure::ResponseEnd,
        Measure::BargeInStop,
    ] {
        assert!(measure.applies(Topology::Native), "{measure:?}");
        assert!(measure.applies(Topology::Cascade), "{measure:?}");
    }
    for measure in [
        Measure::FinalTranscript,
        Measure::FirstToken,
        Measure::TtsFirstAudio,
        Measure::TtsOfferToAudio,
    ] {
        assert!(!measure.applies(Topology::Native), "{measure:?}");
        assert!(measure.applies(Topology::Cascade), "{measure:?}");
    }
    assert!(Measure::FirstTranscript.applies(Topology::Native));
    assert!(!Measure::FirstTranscript.applies(Topology::Cascade));
    assert_eq!(Topology::Native.name(), "native");
    assert_eq!(Topology::Cascade.name(), "cascade");
}
