//! The live test against the hosted Realtime API. It is ignored and needs the `live` feature:
//!
//! ```text
//! cargo test -p inference-openai --features live -- --ignored --nocapture
//! ```
//!
//! The key is resolved through the crate's credential sources, never read by this test itself.
//! By default it is read from the OS keychain through the `secrets` library, scope
//! `default/default/default`, secret `openai-api-key` (another name in
//! `INFERENCE_OPENAI_SECRET_NAME`), where `secretsctl put openai-api-key` stores it. With
//! `INFERENCE_OPENAI_KEY_SOURCE=env` it is read from `OPENAI_API_KEY` instead. The model is
//! `INFERENCE_OPENAI_REALTIME_MODEL`, or the default model. The key is never printed or logged;
//! the test prints the transcripts and the usage the server reported.

use std::num::{NonZeroU16, NonZeroU32};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_openai::{
    CredentialSource, Endpoint, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME,
    OPENAI_KEY_VARIABLE, OutputModality, RealtimeClient, ResponseStatus, ServerEvent,
    SessionConfig, TurnDetection, Usage, WireFormat,
};

const SECRET_NAME_VARIABLE: &str = "INFERENCE_OPENAI_SECRET_NAME";
const KEY_SOURCE_VARIABLE: &str = "INFERENCE_OPENAI_KEY_SOURCE";

/// The speech clip of `tests/fixtures/speech.wav`; `speech.txt` names its generator.
const SPEECH: &[u8] = include_bytes!("fixtures/speech.wav");

/// The source and the secret name the key comes from: the keychain unless the key source
/// variable is `env`.
fn credential() -> (Box<dyn CredentialSource>, String) {
    let name = std::env::var(SECRET_NAME_VARIABLE)
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| OPENAI_KEY_NAME.to_owned());
    let source = std::env::var(KEY_SOURCE_VARIABLE).unwrap_or_default();
    if source == "env" {
        let bindings = [(name.clone(), OPENAI_KEY_VARIABLE.to_owned())];
        (Box::new(EnvironmentSource::new(bindings)), name)
    } else {
        let keychain = KeychainSource::native().expect("the OS keychain opens");
        (Box::new(keychain), name)
    }
}

/// The samples, sample rate and channel count of a PCM16 WAV file.
fn read_wav(bytes: &[u8]) -> (Vec<u8>, u32, u16) {
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut at = 12;
    let mut format = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        if id == b"fmt " {
            assert_eq!(u16::from_le_bytes([body[0], body[1]]), 1, "PCM");
            assert_eq!(u16::from_le_bytes([body[14], body[15]]), 16, "16-bit");
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            format = Some((rate, channels));
        } else if id == b"data" {
            let (rate, channels) = format.expect("fmt before data");
            let whole = body.len() - body.len() % (2 * usize::from(channels));
            return (body[..whole].to_vec(), rate, channels);
        }
        at += 8 + size + size % 2;
    }
    panic!("no data chunk");
}

/// The clip as frames of 100 ms.
fn speech_frames() -> Vec<AudioFrame> {
    let (data, rate, channels) = read_wav(SPEECH);
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::new(channels).unwrap(),
    };
    let frame_bytes = (rate / 10) as usize * 2 * usize::from(channels);
    let mut position = 0u64;
    data.chunks(frame_bytes)
        .map(|chunk| {
            let sample_count = (chunk.len() / (2 * usize::from(channels))) as u32;
            let frame = AudioFrame {
                format,
                position: SampleClockPosition::new(position).unwrap(),
                sample_count,
                data: chunk.to_vec(),
            };
            position += u64::from(sample_count);
            frame
        })
        .collect()
}

/// What one response produced.
#[derive(Default)]
struct Outcome {
    audio_bytes: usize,
    transcript: String,
    input_transcript: Option<String>,
    status: Option<ResponseStatus>,
    usage: Option<Usage>,
}

/// Reads events until `response.done` and, when `want_input` is set, the input transcript.
async fn collect(client: &mut RealtimeClient, want_input: bool) -> Outcome {
    let mut outcome = Outcome::default();
    while outcome.status.is_none() || (want_input && outcome.input_transcript.is_none()) {
        match client.next_event().await.expect("a server event") {
            ServerEvent::AudioDelta { audio, .. } => outcome.audio_bytes += audio.len(),
            ServerEvent::OutputTranscriptDelta { delta, .. } => outcome.transcript.push_str(&delta),
            ServerEvent::InputTranscript { transcript, .. } => {
                outcome.input_transcript = Some(transcript);
            }
            ServerEvent::ResponseDone { status, usage, .. } => {
                outcome.status = Some(status);
                outcome.usage = usage;
            }
            ServerEvent::Error(error) => panic!("server error: {error}"),
            _ => {}
        }
    }
    outcome
}

#[tokio::test]
#[ignore = "calls the hosted OpenAI Realtime API: needs a key and is billed"]
async fn text_in_audio_out_then_speech_in() {
    let (source, name) = credential();
    let endpoint = Endpoint::from_env();
    let session = SessionConfig {
        instructions: Some("Answer in one short sentence.".to_owned()),
        voice: Some("marin".to_owned()),
        output_modality: OutputModality::Audio,
        input_format: WireFormat::Pcm16,
        output_format: WireFormat::Pcm16,
        turn_detection: TurnDetection::Off,
        input_transcription: Some("gpt-4o-mini-transcribe".to_owned()),
        tools: Vec::new(),
    };
    let mut client = RealtimeClient::connect(&endpoint, source.as_ref(), &name, &session)
        .await
        .expect("session setup");
    println!("model: {}", endpoint.model);

    client.send_text("Say hello in five words.").await.unwrap();
    client.create_response().await.unwrap();
    let text = collect(&mut client, false).await;
    println!(
        "text in, audio out: {} audio bytes, transcript {:?}",
        text.audio_bytes, text.transcript
    );
    match &text.usage {
        Some(usage) => println!("text in, audio out usage: {usage}"),
        None => println!("text in, audio out usage: not reported"),
    }
    assert_eq!(text.status, Some(ResponseStatus::Completed));
    assert!(text.audio_bytes > 0, "no audio deltas");
    assert!(!text.transcript.trim().is_empty(), "no output transcript");

    for frame in speech_frames() {
        client.append_audio(&frame).await.unwrap();
    }
    client.commit_audio().await.unwrap();
    client.create_response().await.unwrap();
    let speech = collect(&mut client, true).await;
    println!(
        "audio in: input transcript {:?}; response transcript {:?}",
        speech.input_transcript, speech.transcript
    );
    match &speech.usage {
        Some(usage) => println!("audio in usage: {usage}"),
        None => println!("audio in usage: not reported"),
    }
    assert_eq!(speech.status, Some(ResponseStatus::Completed));
    let heard = speech.input_transcript.unwrap_or_default();
    assert!(!heard.trim().is_empty(), "no input transcript");
    client.close().await.unwrap();
}
