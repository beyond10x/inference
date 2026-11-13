//! The async form of the three speech handles against the testkit mocks: 20 syntheses, 20
//! recognitions and 20 speech sessions at once on one current-thread runtime, without
//! `spawn_blocking`, each delivering every event while a ticker on the same runtime keeps its
//! largest gap under 50 ms; and the TTS, STT and duplex conformance procedures run once more
//! with every event pulled through the async form.

#![cfg(all(feature = "stt", feature = "tts", feature = "duplex"))]

use std::cell::Cell;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_core::envelope::EventEnvelope;
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{Adapter, ModelId};
use inference_duplex::conformance::{NativeDuplexProbe, native_duplex_evidence};
use inference_duplex::inference_audio::{AudioFrame, SampleClockPosition};
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle, TurnDetection,
    TurnTuning,
};
use inference_stt::inference_text::ReviseSpan;
use inference_stt::{
    ConcurrencyLimitReached, InputEnded, Recogniser, RecognitionHandle, RecognitionRequest,
    RevisionRefused, TranscriptEvent, TranscriptOutput,
};
use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn, frame, pcm, saying};
use inference_testkit::stt::{MockRecogniser, frames, script};
use inference_testkit::tts::conformance::Utterance;
use inference_testkit::tts::{MockConfiguration, MockSynthesiser};
use inference_testkit::{stt, tts};
use inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued, TurnId,
};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::{
    DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused, SynthesisEvent,
    SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};

const SESSIONS: usize = 20;
const TICK: Duration = Duration::from_millis(10);
const GAP_BOUND: Duration = Duration::from_millis(50);

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime")
}

/// Runs `sessions` on a current-thread runtime beside a ticker that ticks every [`TICK`] until
/// they are done; returns what they returned and the ticker's largest gap.
fn with_ticker<F, T>(sessions: Vec<F>) -> (Vec<T>, Duration)
where
    F: Future<Output = T> + 'static,
    T: 'static,
{
    let runtime = runtime();
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        let done = Rc::new(Cell::new(false));
        let ticker = {
            let done = Rc::clone(&done);
            tokio::task::spawn_local(async move {
                let mut largest = Duration::ZERO;
                let mut last = Instant::now();
                while !done.get() {
                    tokio::time::sleep(TICK).await;
                    let now = Instant::now();
                    largest = largest.max(now - last);
                    last = now;
                }
                largest
            })
        };
        let running: Vec<_> = sessions.into_iter().map(tokio::task::spawn_local).collect();
        let mut results = Vec::new();
        for session in running {
            results.push(session.await.expect("a session task ends without a panic"));
        }
        done.set(true);
        let largest = ticker.await.expect("the ticker ends without a panic");
        (results, largest)
    })
}

fn format() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("16000 is not zero"),
        channels: NonZeroU16::new(1).expect("one is not zero"),
    }
}

fn segments(texts: &[&str]) -> Vec<OutputSegment> {
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    texts
        .iter()
        .zip(1..)
        .map(|(text, sequence)| {
            let (tentative, _) =
                OutputSegment::emit(&generation, sequence, (*text).to_owned()).expect("emitted");
            tentative.commit().expect("a tentative segment commits").0
        })
        .collect()
}

fn request() -> RecognitionRequest {
    RecognitionRequest::new(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        NodeId::generate(),
        "text_out",
    )
}

/// One synthesis of two segments through the async form: every event until the usage record.
async fn synthesise() -> Vec<SynthesisEvent> {
    let mut synthesiser = MockSynthesiser::new(
        MockConfiguration::IncrementalStreaming,
        ModelId::generate(),
        format(),
    );
    let mut handle = synthesiser
        .start()
        .expect("a fresh synthesiser runs nothing");
    let mut events = Vec::new();
    for segment in segments(&["Hello there.", "The weather is fine today."]) {
        handle
            .offer_segment(segment)
            .expect("the utterance is open");
        while let Some(event) = handle.event().await {
            events.push(event);
            tokio::task::yield_now().await;
        }
    }
    handle.end_utterance().expect("the utterance ends once");
    while let Some(event) = handle.event().await {
        events.push(event);
        tokio::task::yield_now().await;
    }
    events
}

#[test]
fn twenty_syntheses_deliver_every_event_through_the_async_form_and_the_ticker_keeps_time() {
    let (results, largest) = with_ticker((0..SESSIONS).map(|_| synthesise()).collect());
    assert_eq!(results.len(), SESSIONS);
    for events in &results {
        assert!(
            matches!(events.last(), Some(SynthesisEvent::Usage(_))),
            "the usage record is the last event: {events:?}"
        );
        let audio = events
            .iter()
            .filter(|event| matches!(event, SynthesisEvent::Audio(_)))
            .count();
        assert_eq!(audio, 7, "one frame per word of the two segments");
    }
    assert!(largest < GAP_BOUND, "the ticker's largest gap {largest:?}");
}

/// One recognition of the mock's script through the async form: every event until the usage
/// record.
async fn recognise() -> Vec<EventEnvelope<TranscriptEvent>> {
    let mut recogniser = MockRecogniser::new(
        stt::MockConfiguration::Incremental,
        ModelId::generate(),
        script(),
    );
    let mut handle = recogniser
        .start(request())
        .expect("a fresh recogniser runs nothing");
    let mut events = Vec::new();
    for frame in frames(6) {
        handle.offer_frame(frame).expect("input is open");
        while let Some(event) = handle.event().await {
            events.push(event);
            tokio::task::yield_now().await;
        }
    }
    handle.end_input().expect("input ends once");
    while let Some(event) = handle.event().await {
        events.push(event);
        tokio::task::yield_now().await;
    }
    events
}

#[test]
fn twenty_recognitions_deliver_every_event_through_the_async_form_and_the_ticker_keeps_time() {
    let (results, largest) = with_ticker((0..SESSIONS).map(|_| recognise()).collect());
    assert_eq!(results.len(), SESSIONS);
    for events in &results {
        assert!(
            matches!(
                events.last().map(|event| &event.payload),
                Some(TranscriptEvent::Usage(_))
            ),
            "the usage record is the last event: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event.payload, TranscriptEvent::Committed(_))),
            "a span is committed: {events:?}"
        );
    }
    assert!(largest < GAP_BOUND, "the ticker's largest gap {largest:?}");
}

fn session_config(turn_detection: TurnDetection, auto: bool) -> SessionConfig {
    SessionConfig {
        voice: "calm".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(16_000),
        output_format: pcm(16_000),
        turn_detection,
        turn_tuning: TurnTuning::default(),
        auto_response: auto,
        auto_interrupt: auto,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

fn turn(heard: &str, reply: &str) -> ScriptedTurn {
    ScriptedTurn {
        heard: heard.to_owned(),
        reply: Reply::Say(reply.to_owned()),
    }
}

/// One session through the async form: an utterance, its whole response, then the close and
/// every event until the async form ends.
async fn converse() -> Vec<SessionEvent> {
    let mut model = MockSpeechModel::native(vec![turn("what time is it", "It is noon")]);
    let mut session = model
        .open(session_config(TurnDetection::ServerVad, true))
        .expect("the mock takes the configuration");
    for index in 0..3_u64 {
        let byte = if index < 2 { 9 } else { 0 };
        session
            .append_audio(frame(pcm(16_000), index * 320, byte))
            .expect("input in the session's format");
    }
    let mut events = Vec::new();
    while let Some(event) = session.event().await {
        let completed = matches!(event, SessionEvent::ResponseCompleted(_));
        events.push(event);
        tokio::task::yield_now().await;
        if completed {
            break;
        }
    }
    session.close().expect("the session is open");
    while let Some(event) = session.event().await {
        events.push(event);
        tokio::task::yield_now().await;
    }
    events
}

#[test]
fn twenty_speech_sessions_deliver_every_event_through_the_async_form_and_the_ticker_keeps_time() {
    let (results, largest) = with_ticker((0..SESSIONS).map(|_| converse()).collect());
    assert_eq!(results.len(), SESSIONS);
    for events in &results {
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SessionEvent::ResponseCompleted(_))),
            "the response completed: {events:?}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::OutputAudio(_)))
                .count(),
            2,
            "two frames of output audio: {events:?}"
        );
        assert!(
            matches!(events.last(), Some(SessionEvent::SpeechSessionClosed(_))),
            "the async form ends after the close: {events:?}"
        );
    }
    assert!(largest < GAP_BOUND, "the ticker's largest gap {largest:?}");
}

/// A synthesiser whose handles yield each event by blocking on the async form of the wrapped
/// handle on a current-thread runtime, so a procedure written against the iterator runs through
/// the async form; `pulled` counts the events that came that way.
struct AsyncSynthesiser<S> {
    inner: S,
    runtime: Arc<tokio::runtime::Runtime>,
    pulled: Arc<std::sync::atomic::AtomicUsize>,
}

struct AsyncSynthesis<H> {
    inner: H,
    runtime: Arc<tokio::runtime::Runtime>,
    pulled: Arc<std::sync::atomic::AtomicUsize>,
}

impl<S: Synthesiser> Synthesiser for AsyncSynthesiser<S> {
    type Handle = AsyncSynthesis<S::Handle>;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(&mut self) -> Result<Self::Handle, inference_tts::ConcurrencyLimitReached> {
        Ok(AsyncSynthesis {
            inner: self.inner.start()?,
            runtime: Arc::clone(&self.runtime),
            pulled: Arc::clone(&self.pulled),
        })
    }
}

impl<H: SynthesisHandle> Iterator for AsyncSynthesis<H> {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        let event = self.runtime.block_on(self.inner.event());
        if event.is_some() {
            self.pulled
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        event
    }
}

impl<H: SynthesisHandle> SynthesisHandle for AsyncSynthesis<H> {
    fn output(&self) -> &SynthesisOutput {
        self.inner.output()
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        self.inner.offer_segment(segment)
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

/// The observation `result` holds, or a panic naming `configuration` and the failure.
fn passed<T, E: std::fmt::Display>(configuration: impl std::fmt::Debug, result: Result<T, E>) -> T {
    result.unwrap_or_else(|failure| panic!("{configuration:?}: {failure}"))
}

fn counter() -> Arc<std::sync::atomic::AtomicUsize> {
    Arc::new(std::sync::atomic::AtomicUsize::new(0))
}

#[test]
fn the_tts_conformance_passes_through_the_async_form_for_every_mock_configuration() {
    let runtime = Arc::new(runtime());
    let pulled = counter();
    for configuration in MockConfiguration::ALL {
        let through = || AsyncSynthesiser {
            inner: MockSynthesiser::new(configuration, ModelId::generate(), format()),
            runtime: Arc::clone(&runtime),
            pulled: Arc::clone(&pulled),
        };
        let utterance = Utterance::default();
        passed(
            configuration,
            tts::conformance::usage_recorded(&mut through(), &utterance),
        );
        passed(
            configuration,
            tts::conformance::tentative_refused(&mut through(), &utterance),
        );
        passed(
            configuration,
            tts::conformance::played_only_by_sink(&mut through(), &utterance),
        );
        passed(
            configuration,
            tts::conformance::claims_observed(&mut through(), &utterance),
        );
        passed(
            configuration,
            tts::conformance::audio_valid(&mut through(), &utterance, Some(format())),
        );
        passed(
            configuration,
            tts::conformance::cancellation(&mut through(), &utterance),
        );
    }
    assert!(
        pulled.load(std::sync::atomic::Ordering::Relaxed) > 0,
        "events came through the async form"
    );
}

/// A recogniser whose handles yield each event by blocking on the async form of the wrapped
/// handle on a current-thread runtime.
struct AsyncRecogniser<R> {
    inner: R,
    runtime: Arc<tokio::runtime::Runtime>,
    pulled: Arc<std::sync::atomic::AtomicUsize>,
}

struct AsyncRecognition<H> {
    inner: H,
    runtime: Arc<tokio::runtime::Runtime>,
    pulled: Arc<std::sync::atomic::AtomicUsize>,
}

impl<R: Recogniser> Recogniser for AsyncRecogniser<R> {
    type Handle = AsyncRecognition<R::Handle>;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(
        &mut self,
        request: RecognitionRequest,
    ) -> Result<Self::Handle, ConcurrencyLimitReached> {
        Ok(AsyncRecognition {
            inner: self.inner.start(request)?,
            runtime: Arc::clone(&self.runtime),
            pulled: Arc::clone(&self.pulled),
        })
    }
}

impl<H: RecognitionHandle> Iterator for AsyncRecognition<H> {
    type Item = EventEnvelope<TranscriptEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        let event = self.runtime.block_on(self.inner.event());
        if event.is_some() {
            self.pulled
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        event
    }
}

impl<H: RecognitionHandle> RecognitionHandle for AsyncRecognition<H> {
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

#[test]
fn the_stt_conformance_passes_through_the_async_form_for_every_mock_configuration() {
    let runtime = Arc::new(runtime());
    let pulled = counter();
    for configuration in stt::MockConfiguration::ALL {
        let through = || AsyncRecogniser {
            inner: MockRecogniser::new(configuration, ModelId::generate(), script()),
            runtime: Arc::clone(&runtime),
            pulled: Arc::clone(&pulled),
        };
        let input = stt::conformance::Input::default();
        passed(
            configuration,
            stt::conformance::committed_final(&mut through(), &input),
        );
        passed(
            configuration,
            stt::conformance::spans_in_order(&mut through(), &input),
        );
        passed(
            configuration,
            stt::conformance::end_of_input(&mut through(), &input),
        );
        passed(
            configuration,
            stt::conformance::cancellation(&mut through(), &input),
        );
        passed(
            configuration,
            stt::conformance::usage_recorded(&mut through(), &input),
        );
        passed(
            configuration,
            stt::conformance::claims_observed(&mut through(), &input),
        );
    }
    assert!(
        pulled.load(std::sync::atomic::Ordering::Relaxed) > 0,
        "events came through the async form"
    );
}

/// A native speech model whose sessions wait for each event on the async form of the wrapped
/// session, on a current-thread runtime, for at most the wait asked.
struct AsyncModel<M> {
    inner: M,
    runtime: Arc<tokio::runtime::Runtime>,
    pulled: Arc<std::sync::atomic::AtomicUsize>,
}

struct AsyncSession<S> {
    inner: S,
    runtime: Arc<tokio::runtime::Runtime>,
    pulled: Arc<std::sync::atomic::AtomicUsize>,
}

impl<M: NativeSpeechModel> NativeSpeechModel for AsyncModel<M> {
    type Session = AsyncSession<M::Session>;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        self.inner.capabilities()
    }

    fn open(&mut self, config: SessionConfig) -> Result<Self::Session, OpenError> {
        Ok(AsyncSession {
            inner: self.inner.open(config)?,
            runtime: Arc::clone(&self.runtime),
            pulled: Arc::clone(&self.pulled),
        })
    }
}

impl<S: SpeechSessionHandle> SpeechSessionHandle for AsyncSession<S> {
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
        let inner = &mut self.inner;
        let event = self.runtime.block_on(async move {
            tokio::time::timeout(wait, inner.event())
                .await
                .ok()
                .flatten()
        });
        if event.is_some() {
            self.pulled
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        event
    }
}

#[test]
fn the_duplex_conformance_passes_through_the_async_form() {
    let runtime = Arc::new(runtime());
    let pulled = counter();
    let mut probe_config = session_config(TurnDetection::ServerVad, true);
    probe_config.auto_interrupt = false;
    let probe = NativeDuplexProbe {
        config: probe_config,
        opening: vec![
            saying(pcm(16_000), 0, "what is the weather"),
            frame(pcm(16_000), 320, 0),
        ],
        during: vec![
            saying(pcm(16_000), 640, "in Lisbon"),
            frame(pcm(16_000), 960, 0),
        ],
        marker: "lisbon".to_owned(),
        wait: Duration::from_millis(100),
        patience: 20,
    };
    let mut model = AsyncModel {
        inner: MockSpeechModel::native(vec![
            turn("a question", "The weather is"),
            turn("a place", " mild {heard}"),
        ]),
        runtime: Arc::clone(&runtime),
        pulled: Arc::clone(&pulled),
    };
    let evidence = native_duplex_evidence(&mut model, &probe)
        .expect("the native mock answers input during output on the same response");
    assert_eq!(evidence.before, "The weather is");
    assert_eq!(evidence.after, " mild in Lisbon");
    assert!(
        pulled.load(std::sync::atomic::Ordering::Relaxed) > 0,
        "events came through the async form"
    );
}
