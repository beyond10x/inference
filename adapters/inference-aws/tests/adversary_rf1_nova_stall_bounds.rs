//! Adversary pass 1 on story:nova-sonic-stall-and-bounds: the variants the unit's own tests do
//! not build. Offline: every provider is a local channel, no credential, no network.
#![cfg(feature = "duplex")]

use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use inference_aws::duplex::{Connector, NovaSonicSpeech};
use inference_aws::nova_sonic::{
    DEFAULT_MODEL_ID, NovaSonicError, OutputEvent, Session, SessionConfig as ClientConfig,
};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SessionFailureKind, SpeechSessionHandle,
    TurnDetection, TurnTuning,
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

fn speech() -> AudioFrame {
    AudioFrame {
        format: pcm(16_000),
        position: SampleClockPosition::new(0).expect("a position"),
        sample_count: 320,
        data: vec![9; 640],
    }
}

fn config(idle_limit: Duration) -> SessionConfig {
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
        idle_limit,
    }
}

fn event(name: &str, body: Value) -> Vec<u8> {
    json!({"event": {name: body}}).to_string().into_bytes()
}

/// A provider that sends `frames` through an output channel of `capacity`, after the first audio
/// input when `after_audio`, then holds the stream open, or closes it at once when `frames` is
/// empty and `close_at_once`. `sent` counts the frames the session took; `closed` is set once a
/// send finds the session's side gone.
#[derive(Clone)]
struct Provider {
    frames: Arc<Vec<Vec<u8>>>,
    capacity: usize,
    after_audio: bool,
    close_at_once: bool,
    sent: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
}

impl Provider {
    fn new(frames: Vec<Vec<u8>>, capacity: usize, after_audio: bool) -> Self {
        Provider {
            frames: Arc::new(frames),
            capacity,
            after_audio,
            close_at_once: false,
            sent: Arc::new(AtomicUsize::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Connector for Provider {
    fn model_id(&self) -> &str {
        DEFAULT_MODEL_ID
    }

    fn connect(
        &self,
        config: ClientConfig,
    ) -> impl Future<Output = Result<Session, NovaSonicError>> + Send + 'static {
        let provider = self.clone();
        async move {
            let (input, mut received) = mpsc::channel::<Vec<u8>>(1024);
            let (frames, output) = mpsc::channel(provider.capacity);
            let session = Session::open(&config, input, output).await?;
            tokio::spawn(async move {
                if provider.close_at_once {
                    drop(frames);
                    while received.recv().await.is_some() {}
                    return;
                }
                if provider.after_audio {
                    while let Some(bytes) = received.recv().await {
                        if String::from_utf8_lossy(&bytes).contains("audioInput") {
                            break;
                        }
                    }
                }
                for frame in provider.frames.iter() {
                    if frames.send(Ok(frame.clone())).await.is_err() {
                        provider.closed.store(true, Ordering::SeqCst);
                        return;
                    }
                    provider.sent.fetch_add(1, Ordering::SeqCst);
                }
                while received.recv().await.is_some() {}
                drop(frames);
            });
            Ok(session)
        }
    }
}

/// W3: audio the writer sends after a stall holds off the next stall until one read timeout
/// after the last of it, so the shared activity still drives liveness once the reader has its
/// own stall time.
#[tokio::test]
async fn adversary_rf1_writer_audio_after_a_stall_holds_off_the_next_stall() {
    let (input, _received) = mpsc::channel(1024);
    let (_frames, output) = mpsc::channel(4);
    let mut client = ClientConfig::new("quiet");
    client.read_timeout = Duration::from_millis(100);
    let Session {
        mut writer,
        mut reader,
    } = Session::open(&client, input, output)
        .await
        .expect("the session opens over channels");
    assert!(matches!(
        reader.recv().await,
        Err(NovaSonicError::Stall { .. })
    ));
    let stalled = Instant::now();
    let speaking = tokio::spawn(async move {
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(400) {
            writer.send_audio(&[0; 640]).await.expect("audio sent");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        writer
    });
    let next = tokio::time::timeout(Duration::from_secs(3), reader.recv())
        .await
        .expect("a quiet reader stalls again");
    let waited = stalled.elapsed();
    let _writer = speaking.await.expect("the writer task ends");
    assert!(
        matches!(next, Err(NovaSonicError::Stall { .. })),
        "{next:?}"
    );
    assert!(
        waited >= Duration::from_millis(460),
        "story:nova-sonic-stall-and-bounds: the next stall came {waited:?} after the first while \
         the writer sent audio for 400 ms at a 100 ms read timeout"
    );
}

/// W3: a frame the reader receives after a stall restarts the clock from that frame, not from
/// the stall.
#[tokio::test]
async fn adversary_rf1_a_frame_after_a_stall_restarts_the_clock_from_the_frame() {
    let (input, _received) = mpsc::channel(1024);
    let (frames, output) = mpsc::channel(4);
    let mut client = ClientConfig::new("quiet");
    client.read_timeout = Duration::from_millis(100);
    let Session {
        writer: _writer,
        mut reader,
    } = Session::open(&client, input, output)
        .await
        .expect("the session opens over channels");
    assert!(matches!(
        reader.recv().await,
        Err(NovaSonicError::Stall { .. })
    ));
    tokio::time::sleep(Duration::from_millis(70)).await;
    frames
        .send(Ok(event("somethingNew", json!({}))))
        .await
        .expect("the reader holds its side");
    let framed = Instant::now();
    assert!(
        matches!(reader.recv().await, Ok(Some(OutputEvent::Unknown { .. }))),
        "the frame is read"
    );
    let next = tokio::time::timeout(Duration::from_secs(3), reader.recv())
        .await
        .expect("a quiet reader stalls again");
    let waited = framed.elapsed();
    assert!(
        matches!(next, Err(NovaSonicError::Stall { .. })),
        "{next:?}"
    );
    assert!(
        waited >= Duration::from_millis(90),
        "story:nova-sonic-stall-and-bounds: the stall came {waited:?} after a frame at a 100 ms \
         read timeout"
    );
}

/// N9: with a provider channel of one, a caller that stops reading holds the binding at its bound
/// of 128 arrivals, plus the one the reader holds and the one in the channel; dropping the
/// session ends the reader, so the provider's next send finds the session gone.
#[test]
fn adversary_rf1_arrivals_hold_at_exactly_their_bound_and_end_with_the_session() {
    let total = 5_000;
    let provider = Provider::new(vec![event("somethingNew", json!({})); total], 1, false);
    let mut model = NovaSonicSpeech::with_connector(provider.clone()).expect("the runtime starts");
    let session = model.open(config(Duration::from_secs(5))).expect("opens");
    std::thread::sleep(Duration::from_millis(500));
    let held = provider.sent.load(Ordering::SeqCst);
    assert!(
        (128..=131).contains(&held),
        "story:nova-sonic-stall-and-bounds: {held} frames taken at an arrivals bound of 128 and a \
         provider channel of 1"
    );
    drop(session);
    let dropped = Instant::now();
    while !provider.closed.load(Ordering::SeqCst) && dropped.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        provider.closed.load(Ordering::SeqCst),
        "story:nova-sonic-stall-and-bounds: the reader still held the stream 2 s after the \
         session was dropped"
    );
}

fn fields(mut body: Value) -> Value {
    body["sessionId"] = json!("s");
    body["promptName"] = json!("p");
    body["completionId"] = json!("c");
    body
}

fn audio_block(id: &str, rate: u32) -> Vec<u8> {
    event(
        "contentStart",
        fields(json!({
            "contentId": id,
            "type": "AUDIO",
            "role": "ASSISTANT",
            "audioOutputConfiguration": {
                "mediaType": "audio/lpcm",
                "sampleRateHertz": rate,
                "sampleSizeBits": 16,
                "channelCount": 1,
                "encoding": "base64"
            }
        })),
    )
}

/// N11: a later audio block at another rate fails the session as a provider error, after the
/// first block's audio at the asked rate was delivered.
#[test]
fn adversary_rf1_a_later_block_at_another_rate_fails_the_session() {
    let frames = vec![
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
        audio_block("audio-1", 16_000),
        event(
            "audioOutput",
            fields(json!({"contentId": "audio-1", "content": STANDARD.encode([0_u8; 640])})),
        ),
        event(
            "contentEnd",
            fields(json!({"contentId": "audio-1", "type": "AUDIO", "stopReason": "PARTIAL_TURN"})),
        ),
        audio_block("audio-2", 24_000),
        event(
            "audioOutput",
            fields(json!({"contentId": "audio-2", "content": STANDARD.encode([0_u8; 960])})),
        ),
    ];
    let provider = Provider::new(frames, 8, true);
    let mut model = NovaSonicSpeech::with_connector(provider).expect("the runtime starts");
    let mut session = model.open(config(Duration::from_secs(5))).expect("opens");
    session
        .append_audio(speech())
        .expect("input in the session's format");
    let mut events = Vec::new();
    let started = Instant::now();
    let failure = loop {
        match session.next_event(Duration::from_millis(50)) {
            Some(SessionEvent::SpeechSessionFailed(failed)) => break failed.failure,
            Some(event) => events.push(event),
            None => {}
        }
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "story:nova-sonic-stall-and-bounds: the session never failed: {events:#?}"
        );
    };
    assert_eq!(failure.kind, SessionFailureKind::ProviderError, "{failure}");
    assert!(failure.detail.contains("24000"), "{failure}");
    let delivered = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::OutputAudio(_)))
        .count();
    assert!(
        delivered >= 1,
        "the first block's audio is delivered: {events:#?}"
    );
}

fn closing_provider() -> Provider {
    let mut provider = Provider::new(Vec::new(), 1, false);
    provider.close_at_once = true;
    provider
}

fn failure_within(
    session: &mut impl SpeechSessionHandle,
    limit: Duration,
) -> Option<SessionFailureKind> {
    let started = Instant::now();
    while started.elapsed() < limit {
        if let Some(SessionEvent::SpeechSessionFailed(failed)) =
            session.next_event(Duration::from_millis(50))
        {
            return Some(failed.failure.kind);
        }
    }
    None
}

/// Control for the next case: a provider that closes its stream at once fails the session as a
/// closed transport.
#[test]
fn adversary_rf1_a_closed_stream_fails_the_session() {
    let mut model =
        NovaSonicSpeech::with_connector(closing_provider()).expect("the runtime starts");
    let mut session = model.open(config(Duration::from_secs(5))).expect("opens");
    assert_eq!(
        failure_within(&mut session, Duration::from_secs(2)),
        Some(SessionFailureKind::TransportClosed)
    );
}

/// Boundary of the stall clock: the ledger admits an idle limit of `Duration::MAX`, and the
/// reader's deadline is the last activity plus that limit. A closed stream must still fail the
/// session as a closed transport.
#[test]
fn adversary_rf1_an_idle_limit_of_duration_max_still_reports_the_closed_stream() {
    let mut model =
        NovaSonicSpeech::with_connector(closing_provider()).expect("the runtime starts");
    let mut session = model
        .open(config(Duration::MAX))
        .expect("the ledger admits it");
    assert_eq!(
        failure_within(&mut session, Duration::from_secs(2)),
        Some(SessionFailureKind::TransportClosed),
        "the reader's stall deadline overflowed: the session never hears the stream's end"
    );
}
