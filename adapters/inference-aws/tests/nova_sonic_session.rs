//! The Nova Sonic session sequence over in-process channels, offline: no network, no credential.
//!
//! The order is the one of "Handling input events with the bidirectional API" of the Amazon Nova 2
//! user guide, <https://docs.aws.amazon.com/nova/latest/nova2-userguide/sonic-input-events.html>
//! (sections "Overview", "Audio Streaming" and "Closing the Session"); the output frames are shapes
//! of <https://docs.aws.amazon.com/nova/latest/nova2-userguide/sonic-output-events.html>; the
//! service errors are those of the `InvokeModelWithBidirectionalStream` API reference,
//! <https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_InvokeModelWithBidirectionalStream.html>.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use inference_aws::nova_sonic::{
    AudioRate, DEFAULT_MODEL_ID, GenerationStage, NovaSonicError, OutputEvent, OutputFrame, Role,
    ServiceError, ServiceErrorKind, Session, SessionConfig, redact, resolve_model_id,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

struct Harness {
    session: Session,
    sent: mpsc::Receiver<Vec<u8>>,
    frames: mpsc::Sender<OutputFrame>,
}

async fn open(config: SessionConfig) -> Harness {
    let (input, sent) = mpsc::channel(64);
    let (frames, output) = mpsc::channel(64);
    let session = Session::open(&config, input, output)
        .await
        .expect("the session opens over channels");
    Harness {
        session,
        sent,
        frames,
    }
}

fn drain(sent: &mut mpsc::Receiver<Vec<u8>>) -> Vec<Value> {
    let mut events = Vec::new();
    while let Ok(bytes) = sent.try_recv() {
        events.push(serde_json::from_slice(&bytes).expect("each sent frame is JSON"));
    }
    events
}

fn name(event: &Value) -> &str {
    event["event"]
        .as_object()
        .and_then(|object| object.keys().next())
        .map(String::as_str)
        .unwrap_or("")
}

fn body(event: &Value) -> &Value {
    &event["event"][name(event)]
}

fn config() -> SessionConfig {
    let mut config = SessionConfig::new("You are brief.");
    config.read_timeout = Duration::from_millis(50);
    config
}

#[tokio::test]
async fn opening_sends_session_prompt_and_system_prompt_in_order() {
    let mut harness = open(config()).await;
    let events = drain(&mut harness.sent);
    let names: Vec<&str> = events.iter().map(name).collect();
    assert_eq!(
        names,
        [
            "sessionStart",
            "promptStart",
            "contentStart",
            "textInput",
            "contentEnd"
        ]
    );
    let prompt = harness.session.writer.prompt_name().to_owned();
    assert!(!prompt.is_empty());
    for event in &events[1..] {
        assert_eq!(body(event)["promptName"], prompt.as_str(), "{event}");
    }
    let system = &events[2]["event"]["contentStart"];
    assert_eq!(system["type"], "TEXT");
    assert_eq!(system["role"], "SYSTEM");
    let content = system["contentName"].as_str().unwrap();
    assert_eq!(events[3]["event"]["textInput"]["contentName"], content);
    assert_eq!(events[3]["event"]["textInput"]["content"], "You are brief.");
    assert_eq!(events[4]["event"]["contentEnd"]["contentName"], content);

    let audio_output = &events[1]["event"]["promptStart"]["audioOutputConfiguration"];
    assert_eq!(audio_output["sampleRateHertz"], 24000);
    assert_eq!(audio_output["voiceId"], "matthew");
    assert!(
        events[0]["event"]["sessionStart"]["inferenceConfiguration"]["maxTokens"].is_u64(),
        "{}",
        events[0]
    );
}

#[tokio::test]
async fn audio_opens_one_content_and_streams_into_it() {
    let mut harness = open(config()).await;
    drain(&mut harness.sent);
    let first = vec![1u8, 0, 2, 0];
    let second = vec![3u8, 0, 4, 0];
    harness.session.writer.send_audio(&first).await.unwrap();
    harness.session.writer.send_audio(&second).await.unwrap();
    let events = drain(&mut harness.sent);
    let names: Vec<&str> = events.iter().map(name).collect();
    assert_eq!(names, ["contentStart", "audioInput", "audioInput"]);
    let start = &events[0]["event"]["contentStart"];
    assert_eq!(start["type"], "AUDIO");
    assert_eq!(start["role"], "USER");
    assert_eq!(start["audioInputConfiguration"]["sampleRateHertz"], 16000);
    let content = start["contentName"].as_str().unwrap();
    for (event, pcm) in events[1..].iter().zip([&first, &second]) {
        let audio = &event["event"]["audioInput"];
        assert_eq!(audio["contentName"], content);
        assert_eq!(
            &STANDARD.decode(audio["content"].as_str().unwrap()).unwrap(),
            pcm
        );
    }

    harness.session.writer.end_audio().await.unwrap();
    let events = drain(&mut harness.sent);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event"]["contentEnd"]["contentName"], content);

    harness.session.writer.send_audio(&first).await.unwrap();
    let events = drain(&mut harness.sent);
    let names: Vec<&str> = events.iter().map(name).collect();
    assert_eq!(names, ["contentStart", "audioInput"]);
    assert_ne!(events[0]["event"]["contentStart"]["contentName"], content);
}

#[tokio::test]
async fn telephony_audio_is_declared_at_eight_kilohertz() {
    let mut config = config();
    config.input_rate = AudioRate::Hz8000;
    let mut harness = open(config).await;
    drain(&mut harness.sent);
    harness.session.writer.send_audio(&[0, 0]).await.unwrap();
    let events = drain(&mut harness.sent);
    assert_eq!(
        events[0]["event"]["contentStart"]["audioInputConfiguration"]["sampleRateHertz"],
        8000
    );
}

#[tokio::test]
async fn a_tool_result_is_its_own_content() {
    let mut harness = open(config()).await;
    drain(&mut harness.sent);
    harness
        .session
        .writer
        .send_tool_result("use-7", "{\"temperature\":21}")
        .await
        .unwrap();
    let events = drain(&mut harness.sent);
    let names: Vec<&str> = events.iter().map(name).collect();
    assert_eq!(names, ["contentStart", "toolResult", "contentEnd"]);
    let start = &events[0]["event"]["contentStart"];
    assert_eq!(start["type"], "TOOL");
    assert_eq!(start["role"], "TOOL");
    assert_eq!(start["toolResultInputConfiguration"]["toolUseId"], "use-7");
    let content = start["contentName"].as_str().unwrap();
    assert_eq!(events[1]["event"]["toolResult"]["contentName"], content);
    assert_eq!(
        events[1]["event"]["toolResult"]["content"],
        "{\"temperature\":21}"
    );
    assert_eq!(events[2]["event"]["contentEnd"]["contentName"], content);
}

#[tokio::test]
async fn closing_ends_open_audio_then_the_prompt_then_the_session() {
    let Harness {
        session,
        mut sent,
        frames: _frames,
    } = open(config()).await;
    drain(&mut sent);
    let Session { mut writer, reader } = session;
    writer.send_audio(&[0, 0]).await.unwrap();
    let audio = drain(&mut sent)[0]["event"]["contentStart"]["contentName"]
        .as_str()
        .unwrap()
        .to_owned();
    let prompt = writer.prompt_name().to_owned();
    writer.close().await.unwrap();
    let events = drain(&mut sent);
    assert_eq!(
        events,
        [
            json!({"event": {"contentEnd": {"promptName": prompt, "contentName": audio}}}),
            json!({"event": {"promptEnd": {"promptName": prompt}}}),
            json!({"event": {"sessionEnd": {}}}),
        ]
    );
    assert_eq!(sent.recv().await, None, "closing drops the input side");
    drop(reader);
}

#[tokio::test]
async fn closing_without_audio_sends_only_the_prompt_and_session_ends() {
    let mut harness = open(config()).await;
    drain(&mut harness.sent);
    let Session { writer, .. } = harness.session;
    writer.close().await.unwrap();
    let names: Vec<String> = drain(&mut harness.sent)
        .iter()
        .map(|event| name(event).to_owned())
        .collect();
    assert_eq!(names, ["promptEnd", "sessionEnd"]);
}

#[tokio::test]
async fn sending_after_the_transport_closed_is_a_closed_error() {
    let mut harness = open(config()).await;
    drop(harness.sent);
    let result = harness.session.writer.send_audio(&[0, 0]).await;
    assert!(matches!(result, Err(NovaSonicError::Closed)), "{result:?}");
}

#[tokio::test]
async fn the_reader_yields_typed_events_then_the_end() {
    let mut harness = open(config()).await;
    let frames = [
        json!({"event": {"completionStart": {"sessionId": "s", "promptName": "p", "completionId": "c"}}}),
        json!({"event": {"contentStart": {
            "sessionId": "s", "promptName": "p", "completionId": "c", "contentId": "u",
            "additionalModelFields": "{\"generationStage\":\"FINAL\"}",
            "type": "TEXT", "role": "USER", "textOutputConfiguration": {"mediaType": "text/plain"}
        }}}),
        json!({"event": {"textOutput": {
            "sessionId": "s", "promptName": "p", "completionId": "c", "contentId": "u",
            "content": "hello"
        }}}),
    ];
    for frame in &frames {
        harness
            .frames
            .send(Ok(frame.to_string().into_bytes()))
            .await
            .unwrap();
    }
    harness
        .frames
        .send(Err(ServiceError::new(
            ServiceErrorKind::Throttling,
            "Too many requests",
        )))
        .await
        .unwrap();
    drop(harness.frames);

    let reader = &mut harness.session.reader;
    assert!(matches!(
        reader.recv().await.unwrap(),
        Some(OutputEvent::CompletionStart(_))
    ));
    assert!(matches!(
        reader.recv().await.unwrap(),
        Some(OutputEvent::ContentStart(_))
    ));
    let Some(OutputEvent::TextOutput(text)) = reader.recv().await.unwrap() else {
        panic!("expected the user transcript");
    };
    assert_eq!(text.role, Some(Role::User));
    assert_eq!(text.stage, Some(GenerationStage::Final));
    assert_eq!(text.content, "hello");
    let Some(OutputEvent::Error(error)) = reader.recv().await.unwrap() else {
        panic!("expected the service error as an event");
    };
    assert_eq!(error.kind, ServiceErrorKind::Throttling);
    assert_eq!(error.message, "Too many requests");
    assert_eq!(reader.recv().await.unwrap(), None);
}

#[tokio::test]
async fn a_silent_service_is_a_stall_after_the_read_timeout() {
    let mut harness = open(config()).await;
    let result = harness.session.reader.recv().await;
    let Err(NovaSonicError::Stall { after }) = result else {
        panic!("expected a stall, got {result:?}");
    };
    assert_eq!(after, Duration::from_millis(50));
    drop(harness.frames);
}

/// The service is silent while the user speaks. The stall clock runs from the later of the last
/// frame sent and the last frame received, so a user turn longer than the read timeout is not a
/// stall; once both sides are quiet for the read timeout, it is.
#[tokio::test]
async fn a_long_user_turn_is_not_a_stall() {
    let mut config = config();
    config.read_timeout = Duration::from_millis(100);
    let Harness {
        session,
        sent: _sent,
        frames,
    } = open(config).await;
    let Session {
        mut writer,
        mut reader,
    } = session;
    let speaker = tokio::spawn(async move {
        for _ in 0..15 {
            writer.send_audio(&[0, 0, 0, 0]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        frames
            .send(Ok(json!({"event": {"completionStart": {
                "sessionId": "s", "promptName": "p", "completionId": "c"
            }}})
            .to_string()
            .into_bytes()))
            .await
            .unwrap();
        (writer, frames)
    });
    let started = std::time::Instant::now();
    let first = reader.recv().await;
    assert!(
        matches!(first, Ok(Some(OutputEvent::CompletionStart(_)))),
        "a 300 ms user turn under a 100 ms read timeout: {first:?} after {:?}",
        started.elapsed()
    );
    let (_writer, _frames) = speaker.await.unwrap();
    let quiet = std::time::Instant::now();
    let second = reader.recv().await;
    assert!(
        matches!(second, Err(NovaSonicError::Stall { after }) if after == Duration::from_millis(100)),
        "{second:?}"
    );
    assert!(
        quiet.elapsed() >= Duration::from_millis(90),
        "{:?}",
        quiet.elapsed()
    );
}

/// A model id given as an inference-profile ARN carries an account id; the live test prints the
/// model id through [`redact`].
#[test]
fn an_inference_profile_arn_is_redacted() {
    let arn =
        "arn:aws:bedrock:us-east-1:123456789012:inference-profile/us.amazon.nova-2-sonic-v1:0";
    let printed = redact(arn);
    assert!(!printed.contains("123456789012"), "{printed}");
    assert_eq!(redact(DEFAULT_MODEL_ID), DEFAULT_MODEL_ID);
}

#[test]
fn service_error_messages_carry_no_account_identifiers() {
    let error = ServiceError::new(
        ServiceErrorKind::Other("AccessDeniedException".into()),
        "User: arn:aws:sts::123456789012:assumed-role/some-role/some-session is not authorized \
         to perform: bedrock:InvokeModel on account 123456789012",
    );
    assert!(!error.message.contains("123456789012"), "{}", error.message);
    assert!(!error.message.contains("some-role"), "{}", error.message);
    assert!(
        error.message.contains("is not authorized"),
        "{}",
        error.message
    );
    assert!(!error.to_string().contains("123456789012"));
}

#[test]
fn the_model_id_defaults_to_nova_2_sonic_and_is_overridable() {
    assert_eq!(DEFAULT_MODEL_ID, "amazon.nova-2-sonic-v1:0");
    assert_eq!(resolve_model_id(None), DEFAULT_MODEL_ID);
    assert_eq!(resolve_model_id(Some("")), DEFAULT_MODEL_ID);
    assert_eq!(
        resolve_model_id(Some("amazon.nova-sonic-v1:0")),
        "amazon.nova-sonic-v1:0"
    );
}

#[test]
fn session_defaults_follow_the_reference() {
    let config = SessionConfig::new("x");
    assert_eq!(config.input_rate, AudioRate::Hz16000);
    assert_eq!(config.output_rate, AudioRate::Hz24000);
    assert_eq!(config.voice_id, "matthew");
    assert!(config.read_timeout > Duration::ZERO);
}

/// The live test's clip: 16 kHz mono PCM16, generated as `tests/fixtures/question-16k.txt` says.
#[test]
fn the_speech_clip_is_sixteen_kilohertz_mono_pcm16() {
    let wav = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/question-16k.wav"
    ))
    .unwrap();
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(&wav[12..16], b"fmt ");
    let channels = u16::from_le_bytes([wav[22], wav[23]]);
    let rate = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]);
    let bits = u16::from_le_bytes([wav[34], wav[35]]);
    assert_eq!((channels, rate, bits), (1, 16000, 16));
    assert_eq!(AudioRate::try_from(rate), Ok(AudioRate::Hz16000));
}
