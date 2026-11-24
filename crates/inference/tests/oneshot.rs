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
use inference::{Binding, Stack, Voice};
use inference_testkit::mark::{self, Mark, MockId};
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
