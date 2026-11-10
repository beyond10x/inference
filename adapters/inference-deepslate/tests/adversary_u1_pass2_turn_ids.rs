//! Adversary pass 2 on `story:duplex-turn-ids`: which VAD commit a Deepslate transcription
//! names.
//!
//! Deepslate's message reference numbers turns with one session counter over the conversation:
//! its `ChatHistory` example holds the user turn `turnId: 3` ("What time is it?") and the
//! assistant turn `turnId: 4` that answers it, `UserTranscriptionResult` carries the turn id of
//! the user turn ("which conversation turn this transcription belongs to", example `turnId: 3`),
//! and `ResponseBegin`, `ModelTextFragment`, `ModelAudioChunk` and `InferenceComplete` carry the
//! id of the assistant turn. The transcription is "sent after the transcription worker finishes
//! processing", so the assistant turn may begin before it comes. This crate's own fixtures number
//! turns the same way (user turn 0, reply 1, user turn 2, reply 3).
//!
//! * The reply to the first user turn begins before that turn's transcription arrives, as the
//!   asynchronous transcription allows. Each transcription names the commit of its own user
//!   turn, and the second commit does not take the first turn's transcription.
//! * Two VAD commits come before any message names a turn, then each turn's transcription in
//!   order. Each names its own commit.

mod support;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_deepslate::inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_deepslate::inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_deepslate::protocol::ClientBound;
use inference_deepslate::{DeepslateOpal, DeepslateOpalSession, Endpoint, OpalConfig};
use support::{KEY, OpalServer, Script, Step, reply, utterance, within_deadline};

const WAIT: Duration = Duration::from_millis(20);

fn pcm(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        voice: "de56bbd1-3ea7-4543-a17c-504664d9116b".to_owned(),
        instructions: "You are a helpful assistant.".to_owned(),
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

fn frame(position: u64) -> AudioFrame {
    AudioFrame {
        format: pcm(16_000),
        position: SampleClockPosition::new(position).unwrap(),
        sample_count: 320,
        data: vec![3; 640],
    }
}

fn open(server: &OpalServer) -> DeepslateOpalSession {
    let endpoint = Endpoint::new("vendor-7", "org-3")
        .with_url(&server.url)
        .with_open_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3))
        .with_ping_interval(Duration::from_secs(1));
    DeepslateOpal::new(
        endpoint,
        OpalConfig::new(),
        Arc::new(support::FixedKey(KEY)),
        "deepslate-api-key",
    )
    .expect("the fixture endpoint is valid")
    .open(config())
    .expect("the session opens")
}

fn transcription(turn_id: u32, text: &str) -> Step {
    Step::Send(ClientBound::UserTranscription {
        turn_id,
        text: text.to_owned(),
        language: "en".to_owned(),
    })
}

/// The commits and transcripts of a session over `steps`, after the four frames 0 to 960, in
/// order, each as `(kind, item, text)` with kind `c` or `t`, until `wanted` of them arrived or
/// five seconds pass.
fn items_of(steps: Vec<Step>, wanted: usize) -> Vec<(char, String, String)> {
    let server = OpalServer::start(Script::new(steps));
    let mut session = open(&server);
    for position in [0, 320, 640, 960] {
        session.append_audio(frame(position)).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut items = Vec::new();
    while Instant::now() < deadline && items.len() < wanted {
        let Some(event) = session.next_event(WAIT) else {
            continue;
        };
        match event {
            SessionEvent::InputCommitted(committed) => {
                items.push(('c', committed.item.to_string(), String::new()));
            }
            SessionEvent::InputTranscribed(heard) => {
                items.push(('t', heard.item.to_string(), heard.text));
            }
            _ => {}
        }
    }
    session.close().unwrap();
    items
}

/// The item of the commit `nth` (from 0) and the item of the transcript saying `text`.
fn commit_and_transcript(
    items: &[(char, String, String)],
    nth: usize,
    text: &str,
) -> (String, String) {
    let commits: Vec<&String> = items
        .iter()
        .filter(|(kind, _, _)| *kind == 'c')
        .map(|(_, item, _)| item)
        .collect();
    let heard: Vec<&String> = items
        .iter()
        .filter(|(kind, _, said)| *kind == 't' && said == text)
        .map(|(_, item, _)| item)
        .collect();
    assert_eq!(commits.len(), 2, "two VAD commits: {items:?}");
    assert_eq!(heard.len(), 1, "{text:?} is transcribed once: {items:?}");
    (commits[nth].clone(), heard[0].clone())
}

/// The reply to the first user turn (assistant turn 1) begins and ends before the transcription
/// of user turn 0 arrives. That transcription names the first commit, and the second user turn's
/// transcription names the second commit.
#[test]
fn a_transcription_after_its_turns_reply_began_names_its_own_commit() {
    within_deadline(|| {
        let mut steps = vec![Step::AwaitAudio(4)];
        steps.extend(utterance(0, 320));
        steps.extend(reply(1, "It is noon.", 1, 640));
        steps.push(transcription(0, "What time is it?"));
        steps.extend(utterance(640, 960));
        steps.push(transcription(2, "And the date?"));
        steps.extend(reply(3, "The tenth.", 1, 640));
        let items = items_of(steps, 4);
        let (first_commit, first_heard) = commit_and_transcript(&items, 0, "What time is it?");
        assert_eq!(
            first_heard, first_commit,
            "the transcription of user turn 0 names the first commit, though assistant turn 1 \
             began before it came: {items:?}"
        );
        let (second_commit, second_heard) = commit_and_transcript(&items, 1, "And the date?");
        assert_eq!(
            second_heard, second_commit,
            "the transcription of user turn 2 names the second commit: {items:?}"
        );
    });
}

/// Two VAD commits come before any message naming a turn; then user turn 0 and user turn 2 are
/// transcribed in order. The first transcription names the first commit.
#[test]
fn transcriptions_after_two_commits_name_the_commits_in_order() {
    within_deadline(|| {
        let mut steps = vec![Step::AwaitAudio(4)];
        steps.extend(utterance(0, 320));
        steps.extend(utterance(640, 960));
        steps.push(transcription(0, "What time"));
        steps.push(transcription(2, "is it?"));
        steps.extend(reply(3, "It is noon.", 1, 640));
        let items = items_of(steps, 4);
        let (first_commit, first_heard) = commit_and_transcript(&items, 0, "What time");
        assert_eq!(
            first_heard, first_commit,
            "the earlier turn's transcription names the earlier commit: {items:?}"
        );
        let (second_commit, second_heard) = commit_and_transcript(&items, 1, "is it?");
        assert_eq!(
            second_heard, second_commit,
            "the later turn's transcription names the later commit: {items:?}"
        );
    });
}
