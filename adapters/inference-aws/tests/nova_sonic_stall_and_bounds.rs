//! Nova Sonic's reader and binding when nothing happens or too much does, offline: a quiet
//! session, a caller that stops reading, a server that never answers the stream's opening, and
//! an answer at a rate the session did not ask for. No credential; the one network peer is a
//! local listener that never answers.
#![cfg(feature = "duplex")]

use std::future::Future;
use std::net::TcpListener;
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use aws_sdk_bedrockruntime::config::{Credentials, Region};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use inference_aws::duplex::{Connector, NovaSonicSpeech, OPEN_TIMEOUT};
use inference_aws::nova_sonic::{
    self, DEFAULT_MODEL_ID, NovaSonic, NovaSonicError, Session, SessionConfig as ClientConfig,
};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::{
    NativeSpeechModel, OpenError, SessionConfig, SessionEvent, SessionFailureKind,
    SpeechSessionHandle, SpeechSessionState, TurnDetection, TurnTuning,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

fn pcm(sample_rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(sample_rate).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

/// A 20 ms frame of speech at 16 kHz.
fn speech() -> AudioFrame {
    let format = pcm(16_000);
    AudioFrame {
        format,
        position: SampleClockPosition::new(0).expect("a position"),
        sample_count: 320,
        data: vec![9; 640],
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        voice: "tiffany".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(16_000),
        output_format: pcm(16_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

/// A provider that sends `frames` as fast as the session takes them, after the first audio
/// input when `after_audio`, and then holds the stream open; `sent` counts the frames the session
/// took.
#[derive(Clone)]
struct Scripted {
    frames: Arc<Vec<Vec<u8>>>,
    after_audio: bool,
    sent: Arc<AtomicUsize>,
}

impl Scripted {
    fn new(frames: Vec<Vec<u8>>, after_audio: bool) -> Self {
        Scripted {
            frames: Arc::new(frames),
            after_audio,
            sent: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn sent(&self) -> usize {
        self.sent.load(Ordering::SeqCst)
    }
}

impl Connector for Scripted {
    fn model_id(&self) -> &str {
        DEFAULT_MODEL_ID
    }

    fn connect(
        &self,
        config: ClientConfig,
    ) -> impl Future<Output = Result<Session, NovaSonicError>> + Send + 'static {
        let scripted = self.clone();
        async move {
            let (input, mut received) = mpsc::channel::<Vec<u8>>(256);
            let (frames, output) = mpsc::channel(8);
            let session = Session::open(&config, input, output).await?;
            tokio::spawn(async move {
                if scripted.after_audio {
                    while let Some(bytes) = received.recv().await {
                        if String::from_utf8_lossy(&bytes).contains("audioInput") {
                            break;
                        }
                    }
                }
                for frame in scripted.frames.iter() {
                    if frames.send(Ok(frame.clone())).await.is_err() {
                        return;
                    }
                    scripted.sent.fetch_add(1, Ordering::SeqCst);
                }
                while received.recv().await.is_some() {}
                drop(frames);
            });
            Ok(session)
        }
    }
}

fn event(name: &str, body: Value) -> Vec<u8> {
    json!({"event": {name: body}}).to_string().into_bytes()
}

/// story:nova-sonic-stall-and-bounds, W3: a reader quiet past its read timeout reports one stall
/// per read timeout over a fixed window, not one per call.
#[tokio::test]
async fn a_quiet_reader_reports_one_stall_per_read_timeout() {
    let (input, _sent) = mpsc::channel(64);
    let (_frames, output) = mpsc::channel(4);
    let mut client = ClientConfig::new("quiet");
    client.read_timeout = Duration::from_millis(50);
    let mut session = Session::open(&client, input, output)
        .await
        .expect("the session opens over channels");
    let window = Duration::from_millis(500);
    let started = Instant::now();
    let mut stalls = 0_u32;
    while started.elapsed() < window {
        match session.reader.recv().await {
            Err(NovaSonicError::Stall { .. }) => stalls += 1,
            other => panic!("a quiet session reads only stalls: {other:?}"),
        }
    }
    assert!(
        (5..=11).contains(&stalls),
        "story:nova-sonic-stall-and-bounds: {stalls} stalls in {window:?} at a 50 ms read timeout"
    );
}

/// story:nova-sonic-stall-and-bounds, N9: a caller that stops calling `next_event` holds what the
/// binding takes from the provider at a bound; reading again takes more.
#[test]
fn a_caller_that_stops_reading_holds_the_arrivals_at_their_bound() {
    let total = 20_000;
    let unknown = event("somethingNew", json!({}));
    let scripted = Scripted::new(vec![unknown; total], false);
    let mut model = NovaSonicSpeech::with_connector(scripted.clone()).expect("the runtime starts");
    let mut session = model.open(config()).expect("opens");
    std::thread::sleep(Duration::from_millis(500));
    let held = scripted.sent();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        scripted.sent(),
        held,
        "the binding read on while nobody read"
    );
    assert!(
        held < 1_000,
        "story:nova-sonic-stall-and-bounds: {held} of {total} frames taken while nobody read"
    );
    for _ in 0..20 {
        assert_eq!(session.next_event(Duration::from_millis(5)), None);
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(scripted.sent() > held, "reading again takes more");
    assert_eq!(session.ledger().session().state(), SpeechSessionState::Open);
}

/// A client of a local server that accepts every connection and never answers.
fn silent_nova() -> NovaSonic {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
    let port = listener.local_addr().expect("its address").port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => held.push(stream),
                Err(_) => return,
            }
        }
    });
    let silent = aws_sdk_bedrockruntime::Config::builder()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .endpoint_url(format!("http://127.0.0.1:{port}"))
        .credentials_provider(Credentials::new(
            "AKIDOFFLINE",
            "offline",
            None,
            None,
            "test",
        ))
        .build();
    NovaSonic::new(
        aws_sdk_bedrockruntime::Client::from_conf(silent),
        nova_sonic::DEFAULT_MODEL_ID,
    )
}

/// story:nova-sonic-stall-and-bounds, A2 of correction 1: the client's own `start`, called
/// directly against a server that accepts the connection and never answers, fails within its
/// deadline as a connect error naming it.
#[tokio::test]
async fn start_against_a_server_that_never_answers_fails_within_its_deadline() {
    let nova = silent_nova().with_open_timeout(Duration::from_millis(300));
    assert_eq!(nova.open_timeout(), Duration::from_millis(300));
    let started = Instant::now();
    let started_session = tokio::time::timeout(
        Duration::from_secs(5),
        nova.start(&ClientConfig::new("quiet")),
    )
    .await
    .expect("story:nova-sonic-stall-and-bounds: start returns within its deadline");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    let Err(NovaSonicError::Connect(message)) = started_session else {
        panic!("start fails to connect: {started_session:?}");
    };
    assert!(message.contains("did not open within"), "{message}");
}

/// story:nova-sonic-stall-and-bounds, N10: a server that accepts the connection and never sends
/// its response headers makes `open` fail within the binding's deadline, as a closed transport.
#[test]
fn a_server_that_never_answers_fails_open_within_the_deadline() {
    let nova = silent_nova();
    let mut model = NovaSonicSpeech::new(nova)
        .expect("the runtime starts")
        .with_open_timeout(Duration::from_millis(300));
    assert_eq!(model.open_timeout(), Duration::from_millis(300));
    let started = Instant::now();
    let (done, result) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(model.open(config()).err());
    });
    let failed = result
        .recv_timeout(Duration::from_secs(5))
        .expect("story:nova-sonic-stall-and-bounds: open returns within its deadline");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    let Some(OpenError::Failed(failure)) = failed else {
        panic!("open fails: {failed:?}");
    };
    assert_eq!(
        failure.kind,
        SessionFailureKind::TransportClosed,
        "{failure}"
    );
    assert!(failure.detail.contains("did not open within"), "{failure}");
}

/// story:nova-sonic-stall-and-bounds, N10: the deadline is stated, and every model starts with it.
#[test]
fn the_open_deadline_is_stated() {
    let model = NovaSonicSpeech::with_connector(Scripted::new(Vec::new(), false))
        .expect("the runtime starts");
    assert_eq!(model.open_timeout(), OPEN_TIMEOUT);
    assert_eq!(OPEN_TIMEOUT, Duration::from_secs(10));
    assert_eq!(silent_nova().open_timeout(), OPEN_TIMEOUT);
}

/// Frames that commit the user's input, then start assistant audio at 24 kHz, for a session
/// asking 16 kHz.
fn other_rate_frames() -> Vec<Vec<u8>> {
    let fields = |body: Value| {
        let mut body = body;
        body["sessionId"] = json!("s");
        body["promptName"] = json!("p");
        body["completionId"] = json!("c");
        body
    };
    vec![
        event("completionStart", fields(json!({}))),
        event(
            "contentStart",
            fields(json!({
                "contentId": "user-1",
                "type": "TEXT",
                "role": "USER",
                "additionalModelFields": "{\"generationStage\":\"FINAL\"}"
            })),
        ),
        event(
            "textOutput",
            fields(json!({"contentId": "user-1", "content": "hi"})),
        ),
        event(
            "contentEnd",
            fields(json!({"contentId": "user-1", "type": "TEXT", "stopReason": "END_TURN"})),
        ),
        event(
            "contentStart",
            fields(json!({
                "contentId": "audio-1",
                "type": "AUDIO",
                "role": "ASSISTANT",
                "audioOutputConfiguration": {
                    "mediaType": "audio/lpcm",
                    "sampleRateHertz": 24_000,
                    "sampleSizeBits": 16,
                    "channelCount": 1,
                    "encoding": "base64"
                }
            })),
        ),
        event(
            "audioOutput",
            fields(json!({"contentId": "audio-1", "content": STANDARD.encode([0_u8; 960])})),
        ),
    ]
}

/// story:nova-sonic-stall-and-bounds, A1 of correction 1: a caller waiting `Duration::MAX` for
/// the next event gets it when it comes; the wait's deadline does not overflow.
#[test]
fn a_wait_of_duration_max_returns_the_next_event() {
    let scripted = Scripted::new(other_rate_frames(), true);
    let mut model = NovaSonicSpeech::with_connector(scripted).expect("the runtime starts");
    let mut session = model.open(config()).expect("opens");
    session
        .append_audio(speech())
        .expect("input in the session's format");
    let (done, waited) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut events = Vec::new();
        while let Some(event) = session.next_event(Duration::MAX) {
            let failed = matches!(event, SessionEvent::SpeechSessionFailed(_));
            events.push(event);
            if failed {
                break;
            }
        }
        let _ = done.send(events);
    });
    let events = waited
        .recv_timeout(Duration::from_secs(5))
        .expect("story:nova-sonic-stall-and-bounds: a wait of Duration::MAX returns");
    assert!(
        matches!(events.last(), Some(SessionEvent::SpeechSessionFailed(_))),
        "{events:#?}"
    );
}

/// story:nova-sonic-stall-and-bounds, N11: assistant audio at a rate other than the session's
/// output rate fails the session as a provider error naming both rates.
#[test]
fn an_answer_at_another_rate_fails_the_session() {
    let scripted = Scripted::new(other_rate_frames(), true);
    let mut model = NovaSonicSpeech::with_connector(scripted).expect("the runtime starts");
    let mut session = model.open(config()).expect("opens");
    session
        .append_audio(speech())
        .expect("input in the session's format");
    let mut events = Vec::new();
    let mut waits = 0;
    let failure = loop {
        match session.next_event(Duration::from_millis(50)) {
            Some(SessionEvent::SpeechSessionFailed(failed)) => break failed.failure,
            Some(event) => events.push(event),
            None => waits += 1,
        }
        assert!(
            waits < 40,
            "story:nova-sonic-stall-and-bounds: the session never failed: {events:#?}"
        );
    };
    assert_eq!(failure.kind, SessionFailureKind::ProviderError, "{failure}");
    assert!(failure.detail.contains("24000"), "{failure}");
    assert!(failure.detail.contains("16000"), "{failure}");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::OutputAudio(_))),
        "{events:#?}"
    );
}
