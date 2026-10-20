//! The usage notes against the hosted OpenAI Realtime API, without a microphone: the speech clip
//! of the OpenAI adapter's live tests is fed to the fake host's microphone, the conversation runs
//! as `talk` runs it, and the usage line of the response and the totals are printed. It is
//! ignored and billed (one short response):
//!
//! ```text
//! cargo test -p realtime-voice --test live_usage -- --ignored --nocapture
//! ```
//!
//! The key is read as `talk` reads it: the OS keychain, secret `openai-api-key`, or with
//! `INFERENCE_OPENAI_KEY_SOURCE=env` the variable `OPENAI_API_KEY`. It is never printed.

use std::time::{Duration, Instant};

use inference_audio_device::fake::FakeHost;
use inference_duplex::inference_tools::Json;
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::{
    CredentialSource, Endpoint, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME,
};
use realtime_voice::{Conversation, Ending, Note, Options, Reported, Tally, Usage, openai_config};
use serde_json::json;

const SPEECH: &[u8] =
    include_bytes!("../../../adapters/inference-openai/tests/fixtures/speech.wav");

/// The fake laptop's input: stereo at 48 kHz.
const RATE: usize = 48_000;

/// The mono PCM16 samples and the sample rate of a WAV file.
fn read_wav(bytes: &[u8]) -> (Vec<i16>, usize) {
    assert_eq!(&bytes[..4], b"RIFF");
    let mut at = 12;
    let mut rate = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        if id == b"fmt " {
            assert_eq!(u16::from_le_bytes([body[2], body[3]]), 1, "mono");
            rate = Some(u32::from_le_bytes(body[4..8].try_into().unwrap()) as usize);
        } else if id == b"data" {
            let samples = body
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair))
                .collect();
            return (samples, rate.expect("fmt before data"));
        }
        at += 8 + size + size % 2;
    }
    panic!("no data chunk");
}

/// The clip at 48 kHz stereo, linearly interpolated, then `silence` of zeros.
fn device_samples(silence: Duration) -> Vec<f32> {
    let (samples, rate) = read_wav(SPEECH);
    let frames = samples.len() * RATE / rate;
    let mut out = Vec::with_capacity(2 * frames);
    for frame in 0..frames {
        let at = frame as f64 * rate as f64 / RATE as f64;
        let index = at as usize;
        let next = (index + 1).min(samples.len() - 1);
        let weight = at - index as f64;
        let sample = f64::from(samples[index]) * (1.0 - weight) + f64::from(samples[next]) * weight;
        let sample = (sample / 32_768.0) as f32;
        out.extend([sample, sample]);
    }
    out.extend(std::iter::repeat_n(
        0.0,
        2 * RATE * silence.as_millis() as usize / 1_000,
    ));
    out
}

fn model() -> OpenAiRealtime {
    let source: Box<dyn CredentialSource> =
        if std::env::var("INFERENCE_OPENAI_KEY_SOURCE").as_deref() == Ok("env") {
            Box::new(EnvironmentSource::openai())
        } else {
            Box::new(KeychainSource::native().expect("the OS keychain opens"))
        };
    OpenAiRealtime::new(Endpoint::from_env(), source, OPENAI_KEY_NAME)
}

#[test]
#[ignore = "calls the hosted Realtime API and is billed; needs a key"]
fn a_spoken_question_prints_its_usage_line_and_the_totals() {
    let host = FakeHost::laptop();
    let mut model = model();
    let clock: Box<dyn Fn() -> Json> = Box::new(|| json!({"local_time": "12:34"}));
    let mut conversation = Conversation::open(
        &mut model,
        openai_config(),
        &host,
        Options::default(),
        clock,
    )
    .expect("the fake devices and the session open");
    let mut tally = Tally::new(Reported::OPENAI);
    let started = Instant::now();
    let mut usage_notes = 0;
    let mut record = |notes: Vec<Note>, tally: &mut Tally| {
        let mut completed = false;
        for note in notes {
            tally.record(&note);
            match &note {
                Note::Heard(text) => println!("you:   {text}"),
                Note::Said { text, .. } => print!("{text}"),
                Note::Usage(usage) => {
                    usage_notes += 1;
                    if let Usage::Response { response_id, .. } = usage {
                        println!(
                            "\n[usage note: {}]",
                            tally.response(*response_id).unwrap_or_default()
                        );
                    } else {
                        println!("[usage note: {usage:?}]");
                    }
                }
                Note::Ended { how, .. } => println!("\n[ended: {how:?}]"),
                other => println!("[{other:?}]"),
            }
            completed |= matches!(
                note,
                Note::Ended {
                    how: Ending::Completed,
                    ..
                }
            );
        }
        completed
    };

    let ten_ms = 2 * RATE / 100;
    for chunk in device_samples(Duration::from_millis(1_500)).chunks(ten_ms) {
        assert!(host.feed(chunk), "the microphone runs");
        let notes = conversation.pump_microphone();
        let _ = record(notes, &mut tally);
    }
    let mut done = false;
    while !done && started.elapsed() < Duration::from_secs(60) && conversation.is_open() {
        done = record(conversation.step(), &mut tally);
        let _ = host.pull(480);
    }
    let _ = record(conversation.close(), &mut tally);

    println!(
        "session: {:.1} s; {}",
        started.elapsed().as_secs_f64(),
        tally.totals()
    );
    assert!(done, "a response completed within a minute");
    assert!(usage_notes > 0, "the session reported usage");
    assert_eq!(tally.totals().unreported, 0, "{}", tally.totals());
}
