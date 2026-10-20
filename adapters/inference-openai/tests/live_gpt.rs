//! The live tests against hosted GPT-Live (`gpt-live-1`). They are ignored, need the `live` and
//! `gpt-live` features and are billed per second of session, so each keeps to one spoken
//! exchange and closes its session at once:
//!
//! ```text
//! cargo test -p inference-openai --features live,gpt-live --test live_gpt -- --ignored --nocapture
//! ```
//!
//! The key is resolved as `tests/live.rs` resolves it: the OS keychain through the `secrets`
//! library, secret `openai-api-key` (another name in `INFERENCE_OPENAI_SECRET_NAME`), or with
//! `INFERENCE_OPENAI_KEY_SOURCE=env` the variable `OPENAI_API_KEY`. The key is never printed or
//! logged.
//!
//! Input is paced as a microphone would send it, one 100 ms frame every 100 ms, speech and then
//! silence, because the guide asks for a continuous stream paced at its sample rate.
//!
//! `the_client_hears_speech_and_answers_with_audio_and_transcript` sends `fixtures/speech.wav`
//! through the raw client and prints the event timeline: when audio arrives, whether it is
//! silent, and how it lines up with the transcripts. That timeline is what the duplex binding's
//! segmentation of output into responses was built on.
//!
//! `one_spoken_exchange_through_the_duplex_contract` sends the same speech through
//! [`OpenAiLive`], paced, expects one response with audio and transcript, and prints the voice
//! seconds and backend usage the session reported.
//!
//! `native_duplex_evidence_of_gpt_live` runs `conformance::native_duplex_evidence`: it sends
//! `fixtures/during.wav` ("And what about Portugal?") while the answer to `speech.wav` plays and
//! looks for "Portugal" on the same response, on the binding's segmentation. The procedure sends
//! each clip in one burst; since GPT-Live's timeline advances with its input, each clip carries
//! trailing silence long enough for the model to answer. The test fails when the model was never
//! put to the test and otherwise passes when the binding's claims agree with the outcome, which
//! it prints either way.

#![cfg(all(feature = "live", feature = "gpt-live"))]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_duplex::conformance::{NativeDuplexProbe, native_duplex_evidence};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_openai::live::{LIVE_MODEL, LiveClient, LiveEvent, LiveFormat, LiveSessionConfig};
use inference_openai::live_duplex::{OpenAiLive, native_duplex_verdict};
use inference_openai::{
    CredentialSource, Endpoint, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME,
    OPENAI_KEY_VARIABLE, encode_pcm16_24k,
};

const SECRET_NAME_VARIABLE: &str = "INFERENCE_OPENAI_SECRET_NAME";
const KEY_SOURCE_VARIABLE: &str = "INFERENCE_OPENAI_KEY_SOURCE";

const SPEECH: &[u8] = include_bytes!("fixtures/speech.wav");
const DURING: &[u8] = include_bytes!("fixtures/during.wav");

/// The bytes of 100 ms of PCM16 at 24 kHz.
const FRAME_BYTES: usize = 4_800;

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

/// The clip as PCM16 at 24 kHz.
fn pcm24(clip: &[u8]) -> Vec<u8> {
    let (data, rate, channels) = read_wav(clip);
    let frame = AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(rate).unwrap(),
            channels: NonZeroU16::new(channels).unwrap(),
        },
        position: SampleClockPosition::new(0).unwrap(),
        sample_count: (data.len() / (2 * usize::from(channels))) as u32,
        data,
    };
    encode_pcm16_24k(&frame).expect("PCM16 converts")
}

/// The largest absolute sample of PCM16 bytes.
fn peak(pcm: &[u8]) -> i32 {
    pcm.as_chunks::<2>()
        .0
        .iter()
        .map(|sample| i32::from(i16::from_le_bytes(*sample)).abs())
        .max()
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "calls hosted GPT-Live: needs a key, billed per second of session"]
async fn the_client_hears_speech_and_answers_with_audio_and_transcript() {
    let (source, name) = credential();
    let endpoint = Endpoint::live(LIVE_MODEL);
    let config = LiveSessionConfig {
        instructions: Some("Answer in two short sentences.".to_owned()),
        voice: Some("marin".to_owned()),
        format: LiveFormat::Pcm24k,
        ..LiveSessionConfig::default()
    };
    let opened = Instant::now();
    let mut client = LiveClient::connect(&endpoint, source.as_ref(), &name, &config)
        .await
        .expect("the session starts");
    println!(
        "started in {} ms: {}",
        opened.elapsed().as_millis(),
        client.session()
    );

    let speech = pcm24(SPEECH);
    let start = Instant::now();
    let at = |instant: Instant| instant.duration_since(start).as_millis();
    let mut sent = 0_usize;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut heard = String::new();
    let mut said = String::new();
    let mut audio_bytes = 0_usize;
    let mut voiced_bytes = 0_usize;
    let mut last_output: Option<Instant> = None;
    let mut last_audio: Option<Instant> = None;
    let mut voicing = false;
    let mut seconds = None;
    let deadline = start + Duration::from_secs(20);
    loop {
        tick.tick().await;
        let frame = if sent < speech.len() {
            let end = (sent + FRAME_BYTES).min(speech.len());
            let mut frame = speech[sent..end].to_vec();
            frame.resize(FRAME_BYTES, 0);
            frame
        } else {
            vec![0_u8; FRAME_BYTES]
        };
        sent += FRAME_BYTES;
        client.append_audio(&frame).await.expect("audio sent");
        loop {
            let event =
                match tokio::time::timeout(Duration::from_millis(5), client.next_event()).await {
                    Err(_) => break,
                    Ok(event) => event.expect("a server event"),
                };
            let now = Instant::now();
            match event {
                LiveEvent::OutputAudio { audio } => {
                    let gap = last_audio.map_or(0, |last| now.duration_since(last).as_millis());
                    let loud = peak(&audio);
                    if gap > 250 || last_audio.is_none() {
                        println!(
                            "t={} out={} audio resumes after {gap} ms: {} bytes, peak {loud}",
                            at(now),
                            audio_bytes / 48,
                            audio.len()
                        );
                    }
                    if (loud > 200) != voicing {
                        voicing = loud > 200;
                        println!(
                            "t={} out={} in={} voice {} (peak {loud}, {} bytes)",
                            at(now),
                            audio_bytes / 48,
                            sent / 48,
                            if voicing { "on" } else { "off" },
                            audio.len()
                        );
                    }
                    audio_bytes += audio.len();
                    if loud > 200 {
                        voiced_bytes += audio.len();
                        last_output = Some(now);
                    }
                    last_audio = Some(now);
                }
                LiveEvent::OutputTranscript {
                    delta,
                    start_ms,
                    end_ms,
                } => {
                    println!(
                        "t={} out={} said {delta:?} [{start_ms}..{end_ms}]",
                        at(now),
                        audio_bytes / 48
                    );
                    said.push_str(&delta);
                    last_output = Some(now);
                }
                LiveEvent::InputTranscript {
                    delta,
                    start_ms,
                    end_ms,
                } => {
                    println!(
                        "t={} in={} heard {delta:?} [{start_ms}..{end_ms}]",
                        at(now),
                        sent / 48
                    );
                    heard.push_str(&delta);
                }
                LiveEvent::UsageUpdated {
                    seconds: total,
                    context_usage_ratio,
                } => {
                    println!(
                        "t={} usage {total} s, context {context_usage_ratio:?}",
                        at(now)
                    );
                    seconds = Some(total);
                }
                other => println!("t={} {other:?}", at(now)),
            }
        }
        let now = Instant::now();
        let quiet =
            last_output.is_some_and(|last| now.duration_since(last) > Duration::from_millis(2_500));
        if (sent >= speech.len() && quiet) || now > deadline {
            break;
        }
    }
    let closing = Instant::now();
    let closed = client.close().await;
    println!("closed in {} ms: {closed:?}", closing.elapsed().as_millis());
    println!("heard: {heard:?}");
    println!("said: {said:?}");
    println!(
        "audio: {audio_bytes} bytes ({} ms), voiced {voiced_bytes} bytes ({} ms)",
        audio_bytes / 48,
        voiced_bytes / 48
    );
    println!("last running usage: {seconds:?} s");
    let closed = closed.expect("session.closed confirms the final usage");
    println!(
        "final usage: {} s, reason {}",
        closed.seconds, closed.reason
    );
    assert!(voiced_bytes > 0, "spoken audio out");
    assert!(!said.trim().is_empty(), "a transcript of the answer");
}

fn pcm24_format() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

/// The clip as 24 kHz PCM16 frames of 100 ms from sample `from`, then `silence` of zeros.
fn clip_frames(clip: &[u8], from: u64, silence: Duration) -> Vec<AudioFrame> {
    let mut pcm = pcm24(clip);
    pcm.extend(std::iter::repeat_n(0_u8, silence.as_millis() as usize * 48));
    let mut at = from;
    pcm.chunks(FRAME_BYTES)
        .map(|chunk| {
            let frame = AudioFrame {
                format: pcm24_format(),
                position: SampleClockPosition::new(at).unwrap(),
                sample_count: (chunk.len() / 2) as u32,
                data: chunk.to_vec(),
            };
            at += u64::from(frame.sample_count);
            frame
        })
        .collect()
}

fn model() -> OpenAiLive {
    let (source, name) = credential();
    OpenAiLive::hosted(source, name)
}

fn contract_config(instructions: &str) -> SessionConfig {
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: instructions.to_owned(),
        input_format: pcm24_format(),
        output_format: pcm24_format(),
        turn_detection: TurnDetection::SemanticVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(30),
    }
}

#[test]
#[ignore = "calls hosted GPT-Live: needs a key, billed per second of session"]
fn one_spoken_exchange_through_the_duplex_contract() {
    let mut model = model();
    let mut session = model
        .open(contract_config("Answer in one short sentence."))
        .expect("the session opens");
    let speech = clip_frames(SPEECH, 0, Duration::ZERO);
    let mut position = speech.last().map_or(0, |frame| {
        frame.position.get() + u64::from(frame.sample_count)
    });
    let mut pending = speech.into_iter();
    let start = Instant::now();
    let mut audio = 0_u64;
    let mut transcript = String::new();
    let mut heard = String::new();
    let mut responses = 0;
    let mut done = None;
    let mut tick = 0_u32;
    while done.is_none() && start.elapsed() < Duration::from_secs(20) {
        let frame = pending.next().unwrap_or_else(|| {
            let frame = AudioFrame {
                format: pcm24_format(),
                position: SampleClockPosition::new(position).unwrap(),
                sample_count: 2_400,
                data: vec![0; FRAME_BYTES],
            };
            position += 2_400;
            frame
        });
        session.append_audio(frame).expect("PCM16 at 24 kHz");
        while let Some(event) = session.next_event(Duration::from_millis(5)) {
            match event {
                SessionEvent::ResponseCreated(_) => responses += 1,
                SessionEvent::OutputAudio(output) => audio += u64::from(output.frame.sample_count),
                SessionEvent::OutputTranscriptDelta(delta) => transcript.push_str(&delta.text),
                SessionEvent::InputTranscribed(input) => heard.push_str(&input.text),
                SessionEvent::ResponseUsageRecorded(usage) => {
                    println!("backend usage on the response: {:?}", usage.usage);
                }
                SessionEvent::ResponseCompleted(completed) => done = Some(completed.response_id),
                SessionEvent::SpeechSessionFailed(failed) => panic!("failed: {}", failed.failure),
                _ => {}
            }
        }
        tick += 1;
        let next = start + Duration::from_millis(100) * tick;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let running = session.voice_seconds();
    session.close().expect("open");
    println!("heard: {heard:?}");
    println!("answered: {transcript:?} ({audio} samples at 24 kHz, {responses} response)");
    println!("voice seconds while open: {running:?}");
    match session.final_usage() {
        Some(closed) => println!(
            "final usage: {} voice seconds, reason {}",
            closed.seconds, closed.reason
        ),
        None => println!("final usage: unconfirmed, no session.closed"),
    }
    for usage in session.backend_usage() {
        println!("backend response usage: {usage}");
    }
    if session.backend_usage().is_empty() {
        println!("backend usage: none reported (no delegation)");
    }
    for error in session.server_errors() {
        println!("server error: {error}");
    }
    assert!(done.is_some(), "one response completed");
    assert_eq!(responses, 1);
    assert!(audio > 0, "audio out");
    assert!(!transcript.trim().is_empty(), "a transcript of the audio");
    assert!(
        session.final_usage().is_some(),
        "session.closed confirms the usage"
    );
}

#[test]
#[ignore = "calls hosted GPT-Live: needs a key, billed per second of session"]
fn native_duplex_evidence_of_gpt_live() {
    let opening = clip_frames(SPEECH, 0, Duration::from_millis(1_000));
    let start = opening.last().map_or(0, |frame| {
        frame.position.get() + u64::from(frame.sample_count)
    });
    let probe = NativeDuplexProbe {
        config: contract_config(
            "Answer in four or five sentences. If the listener adds a question while you speak, \
             take it up.",
        ),
        opening,
        during: clip_frames(DURING, start, Duration::from_secs(10)),
        marker: "portugal".to_owned(),
        wait: Duration::from_millis(500),
        patience: 30,
    };
    let outcome = native_duplex_evidence(&mut model(), &probe);
    println!("native_duplex evidence: {outcome:?}");
    assert_eq!(
        native_duplex_verdict(&outcome),
        Ok(true),
        "the binding claims native_duplex exactly when the hosted model gives the evidence, and \
         only an outcome that put the model to the test decides it"
    );
}
