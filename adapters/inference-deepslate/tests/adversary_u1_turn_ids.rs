//! Adversary pass 1 on `story:duplex-turn-ids`: a Deepslate commit whose transcription never
//! comes.
//!
//! The binding names each VAD commit `turn-N` and gives a transcription of a new `turn_id` the
//! oldest commit no transcription claimed. The story names this shape as the defect it removes:
//! "a queue of committed turns would misplace every transcript after one failed transcription",
//! and its outcome holds "also when a provider ... never sends one". Here the first user turn is
//! committed and never transcribed; the second turn is committed and transcribed right after its
//! own commit. That transcription names the second commit, not the first.

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

#[test]
fn a_commit_never_transcribed_does_not_take_the_next_turns_transcription() {
    within_deadline(|| {
        let mut steps = vec![Step::AwaitAudio(4)];
        steps.extend(utterance(0, 320));
        steps.extend(utterance(640, 960));
        steps.push(Step::Send(ClientBound::UserTranscription {
            turn_id: 2,
            text: "What time is it?".to_owned(),
            language: "en".to_owned(),
        }));
        steps.extend(reply(3, "Noon.", 1, 640));
        let server = OpalServer::start(Script::new(steps));
        let mut session = open(&server);
        for position in [0, 320, 640, 960] {
            session.append_audio(frame(position)).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while Instant::now() < deadline {
            if let Some(event) = session.next_event(WAIT) {
                let done = matches!(event, SessionEvent::ResponseCompleted(_));
                events.push(event);
                if done {
                    break;
                }
            }
        }
        let commits: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::InputCommitted(committed) => Some(committed.item.to_string()),
                _ => None,
            })
            .collect();
        let heard: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::InputTranscribed(heard) => Some(heard.item.to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(commits.len(), 2, "two VAD commits: {events:#?}");
        assert_eq!(
            heard,
            [commits[1].clone()],
            "the transcription that follows the second commit names the second commit, not the \
             first one, whose transcription never came: commits {commits:?}"
        );
        session.close().unwrap();
    });
}

/// Every event of a session over `steps`, after the four frames 0 to 960, until the first
/// response completed or five seconds pass; then the items its commits and its transcripts name,
/// in order, each marked `c` or `t`.
fn items_of(steps: Vec<Step>) -> Vec<(char, String)> {
    let server = OpalServer::start(Script::new(steps));
    let mut session = open(&server);
    for position in [0, 320, 640, 960] {
        session.append_audio(frame(position)).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut items = Vec::new();
    while Instant::now() < deadline {
        let Some(event) = session.next_event(WAIT) else {
            continue;
        };
        match &event {
            SessionEvent::InputCommitted(committed) => {
                items.push(('c', committed.item.to_string()))
            }
            SessionEvent::InputTranscribed(heard) => items.push(('t', heard.item.to_string())),
            SessionEvent::ResponseCompleted(_) => break,
            _ => {}
        }
    }
    session.close().unwrap();
    items
}

fn transcription(turn_id: u32, text: &str) -> Step {
    Step::Send(ClientBound::UserTranscription {
        turn_id,
        text: text.to_owned(),
        language: "en".to_owned(),
    })
}

/// A second transcription of the same `turn_id` names the same commit, and does not take the
/// next commit's turn: the next turn's transcription names the next commit.
#[test]
fn a_second_transcription_of_one_turn_id_names_the_same_commit() {
    within_deadline(|| {
        let mut steps = vec![Step::AwaitAudio(4)];
        steps.extend(utterance(0, 320));
        steps.push(transcription(0, "What time"));
        steps.push(transcription(0, "What time is it?"));
        steps.extend(utterance(640, 960));
        steps.push(transcription(2, "And the date?"));
        steps.extend(reply(3, "Noon.", 1, 640));
        let items = items_of(steps);
        let names: Vec<&str> = items.iter().map(|(_, item)| item.as_str()).collect();
        let kinds: String = items.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, "cttct", "{items:?}");
        assert_eq!(names[1], names[0], "{items:?}");
        assert_eq!(names[2], names[0], "{items:?}");
        assert_eq!(names[4], names[3], "{items:?}");
        assert_ne!(names[3], names[0], "{items:?}");
    });
}

/// A transcription that comes before its VAD commit names the turn that commit then carries.
#[test]
fn a_transcription_before_its_commit_names_the_turn_the_commit_carries() {
    within_deadline(|| {
        let mut steps = vec![Step::AwaitAudio(4)];
        let mut spoken = utterance(0, 320);
        let silence = spoken.pop().expect("the end of speech");
        steps.extend(spoken);
        steps.push(transcription(0, "What time is it?"));
        steps.push(silence);
        steps.extend(reply(1, "Noon.", 1, 640));
        let items = items_of(steps);
        let kinds: String = items.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, "tc", "{items:?}");
        assert_eq!(items[0].1, items[1].1, "{items:?}");
    });
}
