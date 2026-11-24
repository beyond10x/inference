#![cfg(feature = "testing")]
//! The one-call functions and the handles `Chat`, `Transcriber` and `Voice` over the testkit
//! mocks and the fake audio host (`docs/design/sdk.md` §3.1, §3.2, §3.5). Every test takes
//! [`SESSIONS`] first, so no two of them open sessions at once and the row counts are exact; each
//! builds its stack from typed mock bindings and calls a `_with` form or `new_with`, so none
//! reads the process environment.

use std::num::{NonZeroU16, NonZeroU32};

use inference::audio::{AudioEncoding, AudioFormat};
use inference::audio_device::fake::{FakeDevice, FakeHost};
use inference::core::model::ModelId;
use inference::facade::session::{SessionState, SessionsRow};
use inference::text::{Span, SpanState};
use inference::{Binding, Stack, Transcriber, Voice};
use inference_testkit::mark::{self, Mark, MockId};
use inference_testkit::stt::synthetic_audio::tone_silence_tone_silence;
use inference_testkit::stt::{self, MockRecogniser};
use inference_testkit::tts::{self, MockSynthesiser};

/// Held by every test of this file while it runs.
static SESSIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What the mock synthesiser is asked to say: three words, so three frames.
const WORDS: &str = "one two three";

fn pcm16(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).expect("a rate"),
        channels: NonZeroU16::MIN,
    }
}

/// A stack binding `tts` to a mock synthesiser of 16 kHz mono frames.
fn speaking() -> Stack {
    Stack::new().bind(Binding::tts(MockSynthesiser::new(
        tts::MockConfiguration::IncrementalStreaming,
        ModelId::generate(),
        pcm16(16_000),
    )))
}

/// A fake host with one device, a mono 16 kHz speaker: a microphone does not open on it.
fn speaker_only() -> FakeHost {
    let rate = NonZeroU32::new(16_000).expect("a rate");
    FakeHost::new(vec![FakeDevice::output(
        "test speaker",
        rate,
        NonZeroU16::MIN,
        true,
    )])
}

/// The marks of the mock synthesiser's reply to [`WORDS`]: reply 0, frames 0, 1 and 2.
fn reply_marks() -> Vec<Option<Mark>> {
    (0..3)
        .map(|frame| {
            Some(Mark {
                mock: MockId::Synthesiser,
                reply: 0,
                frame,
            })
        })
        .collect()
}

fn rows() -> Vec<SessionsRow> {
    inference::sessions().sessions_view()
}

/// Asserts the view has exactly one row more than `before`, and that this row is `Closed`.
fn one_closed_session_since(before: &[SessionsRow]) {
    let after = rows();
    let new: Vec<&SessionsRow> = after
        .iter()
        .filter(|row| !before.iter().any(|old| old.session_id == row.session_id))
        .collect();
    assert_eq!(after.len(), before.len() + 1, "one row more: {new:#?}");
    assert_eq!(new.len(), 1, "{new:#?}");
    assert_eq!(new[0].state, SessionState::Closed, "{new:#?}");
}

/// A stack binding `stt` to a mock recogniser replaying the fixture's script: two spans.
fn hearing() -> Stack {
    Stack::new().bind(Binding::stt(MockRecogniser::new(
        stt::MockConfiguration::Incremental,
        ModelId::generate(),
        stt::script(),
    )))
}

/// The texts of the two spans [`stt::script`] commits, in order.
const HEARD: [&str; 2] = ["hello there", "how are you"];

/// The recipe `transcribe` opens over an `stt` binding, takes audio and nothing else, and commits
/// the spans of that audio: its STT node is given no turn by any other node.
#[tokio::test(flavor = "multi_thread")]
async fn the_transcribe_recipe_opens_and_commits_the_spans_of_its_audio() {
    use inference::core::envelope::{EventEnvelope, EventId, Timebase};
    use inference::core::id::{NodeId, StreamId};
    use inference::facade::events::SessionEvent;
    use inference::facade::live::LiveSessions;
    use inference::runtime::run::RunState;
    use inference::{Env, Payload, Recipe};

    let _held = SESSIONS.lock().await;
    let stack = hearing().resolve_with(&Env::new()).expect("resolves");
    let mut instantiation = Recipe::transcribe()
        .instantiate(&stack)
        .expect("transcribe plans");
    let sessions = LiveSessions::new();
    let (session, driver) = instantiation
        .open(&stack, &sessions)
        .await
        .expect("the run opens");
    let driving = tokio::spawn(driver.drive());
    let stream_id = StreamId::generate();
    for (sequence, frame) in (1..).zip(tone_silence_tone_silence()) {
        let envelope = EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: session.run_id(),
            session_id: None,
            stream_id,
            sequence,
            producer: NodeId::generate(),
            port: "audio".to_owned(),
            kind: "inference.audio.AudioFrame".to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: Payload::Audio(frame),
        };
        session.send(envelope).await.expect("sent");
    }
    session.close().await.expect("a Ready session drains");
    let mut committed = Vec::new();
    while let Some(event) = session.receive().await {
        if let SessionEvent::TranscriptCommitted(span) = event {
            committed.push(span.text);
        }
    }
    let run = driving.await.expect("joins").expect("the run ends");
    assert_eq!(run.state(), RunState::Completed);
    assert_eq!(committed, HEARD);
}

/// Writes the fixture's audio, 16 kHz mono PCM16, as a WAV file in `dir` and returns its path.
fn fixture_wav(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("fixture.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("the file is created");
    for frame in tone_silence_tone_silence() {
        for sample in frame.data.as_chunks::<2>().0 {
            writer
                .write_sample(i16::from_le_bytes(*sample))
                .expect("a sample is written");
        }
    }
    writer.finalize().expect("the file is finished");
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn transcribe_with_returns_the_committed_text_of_a_wav_file() {
    let _held = SESSIONS.lock().await;
    let dir = tempfile::tempdir().expect("a directory");
    let path = fixture_wav(dir.path());
    let stack = hearing();
    let before = rows();

    let text = inference::transcribe_with(&stack, &path)
        .await
        .expect("transcribes");

    assert_eq!(text, HEARD.join(" "));
    one_closed_session_since(&before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transcriber_returns_the_scripted_spans_committed_and_in_order() {
    let _held = SESSIONS.lock().await;
    let stack = hearing();
    let mut transcriber = Transcriber::new_with(&stack).expect("plans");

    for _ in 0..2 {
        let before = rows();
        let spans = transcriber
            .transcribe_frames(tone_silence_tone_silence())
            .await
            .expect("transcribes");
        let texts: Vec<&str> = spans.iter().map(Span::text).collect();
        assert_eq!(texts, HEARD, "{spans:#?}");
        assert!(
            spans
                .iter()
                .all(|span| span.state() == SpanState::Committed),
            "{spans:#?}"
        );
        let session = transcriber.session().expect("the last call's session");
        assert!(
            spans.iter().all(|span| span.run_id() == session.run_id()),
            "the spans are the session's run's: {spans:#?}"
        );
        one_closed_session_since(&before);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transcriber_describes_transcribe_and_reaches_its_session_and_pipeline() {
    let _held = SESSIONS.lock().await;
    let stack = hearing();
    let mut transcriber = Transcriber::new_with(&stack).expect("plans");

    assert_eq!(transcriber.describe().recipe, "transcribe");
    let planned = transcriber.pipeline().pipeline_id();
    assert!(
        transcriber.describe().pipeline.is_some(),
        "the pipeline {planned} is described"
    );
    assert!(
        transcriber.session().is_none(),
        "no call opened a session yet"
    );

    transcriber
        .transcribe_frames(tone_silence_tone_silence())
        .await
        .expect("transcribes");
    let session = transcriber.session().expect("the last call's session");
    assert!(
        rows()
            .iter()
            .any(|row| row.session_id == session.session_id() && row.state == SessionState::Closed),
        "the session of the call is a closed row of the view"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_is_not_wav_is_refused_by_name_and_opens_no_session() {
    let _held = SESSIONS.lock().await;
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, "not audio").expect("written");
    let stack = hearing();
    let before = rows();

    let error = inference::transcribe_with(&stack, &path)
        .await
        .expect_err("a text file is no recording");

    assert_eq!(error.kind(), inference::ErrorKind::Layer, "{error}");
    assert!(error.to_string().contains("notes.txt"), "{error}");
    assert_eq!(rows().len(), before.len(), "nothing opened");
}

#[tokio::test(flavor = "multi_thread")]
async fn speak_with_plays_every_frame_of_the_reply_on_the_host_with_no_pull() {
    let _held = SESSIONS.lock().await;
    let stack = speaking();
    let host = speaker_only();
    let before = rows();

    inference::speak_with(&stack, WORDS, host.clone())
        .await
        .expect("speaks");

    let played: Vec<Option<Mark>> = host.played_frames().iter().map(mark::decode).collect();
    assert_eq!(played, reply_marks(), "{:#?}", host.played());
    assert!(!host.output_open(), "the speaker is let go");
    one_closed_session_since(&before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_returns_the_frames_of_the_text_and_plays_the_same_frames() {
    let _held = SESSIONS.lock().await;
    let stack = speaking();
    let mut voice = Voice::new_with(&stack).expect("plans");
    let before = rows();

    let frames = voice.synthesise(WORDS).await.expect("synthesises");

    let marks: Vec<Option<Mark>> = frames.iter().map(mark::decode).collect();
    assert_eq!(marks, reply_marks(), "{frames:#?}");
    one_closed_session_since(&before);

    let host = speaker_only();
    let before = rows();
    voice.play(WORDS, host.clone()).await.expect("plays");
    assert_eq!(host.played_frames(), frames);
    one_closed_session_since(&before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_describes_speak_and_reaches_its_session_and_pipeline() {
    let _held = SESSIONS.lock().await;
    let stack = speaking();
    let mut voice = Voice::new_with(&stack).expect("plans");

    assert_eq!(voice.describe().recipe, "speak");
    let planned = voice.pipeline().pipeline_id();
    assert!(
        voice.describe().pipeline.is_some(),
        "the pipeline {planned} is described"
    );
    assert!(voice.session().is_none(), "no call opened a session yet");

    voice.synthesise(WORDS).await.expect("synthesises");
    let session = voice.session().expect("the last call's session");
    assert!(
        rows()
            .iter()
            .any(|row| row.session_id == session.session_id() && row.state == SessionState::Closed),
        "the session of the call is a closed row of the view"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_voice_without_a_synthesiser_is_refused_when_it_is_built() {
    let _held = SESSIONS.lock().await;
    let before = rows();
    let error = Voice::new_with(&Stack::new()).expect_err("no tts is bound");
    assert_eq!(error.kind(), inference::ErrorKind::RoleUnbound, "{error}");
    assert_eq!(rows().len(), before.len(), "nothing opened");
}
