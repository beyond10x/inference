//! Adversary cases for story:elevenlabs-realtime-stt-adapter: how a recognition ends once input
//! has ended, against the local fixture server.
#![cfg(feature = "stt")]

mod stt_support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_elevenlabs::{
    CommitStrategy, ElevenLabsRecogniser, Endpoint, InputFormat, TranscriptionConfig, VadSettings,
};
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::usage::QuantityKind;
use inference_stt::{Recogniser, RecognitionHandle, TranscriptEvent};
use stt_support::{
    Action, FixedKey, FixtureServer, KEY, Script, Step, Trigger, describe, manual, mulaw, partial,
    request, session_started, wait_for,
};

fn send(on: Trigger, message: String) -> Step {
    Step {
        on,
        action: Action::Send(message),
    }
}

fn wait(on: Trigger, ms: u64) -> Step {
    Step {
        on,
        action: Action::Wait(Duration::from_millis(ms)),
    }
}

fn committed(text: &str, start: f64, end: f64) -> String {
    serde_json::json!({
        "message_type": "committed_transcript_with_timestamps",
        "text": text,
        "words": [{"text": text, "start": start, "end": end, "type": "word", "logprob": -0.1}]
    })
    .to_string()
}

fn usage_final(events: &[EventEnvelope<TranscriptEvent>]) -> Option<(Option<i64>, bool)> {
    match events.last().map(|event| &event.payload) {
        Some(TranscriptEvent::Usage(record)) => Some((
            record
                .quantities
                .iter()
                .find(|quantity| quantity.kind() == QuantityKind::InputAudioMs)
                .and_then(|quantity| quantity.value()),
            record.r#final,
        )),
        _ => None,
    }
}

/// `ElevenLabsError::Stalled` is documented as "No message arrived for the read timeout" and
/// displays as "no server message within {timeout}". A server that answers the end-of-input
/// commit after streaming a partial every 700 ms, with a read timeout of 2 s, was never silent
/// for the read timeout, and its committed transcript must be delivered.
#[test]
fn a_server_that_keeps_talking_after_the_end_of_input_is_not_reported_as_stalled() {
    let server = FixtureServer::start(Script::Steps(vec![
        send(Trigger::Open, session_started()),
        send(Trigger::Audio(1), partial("eins")),
        wait(Trigger::Commit(1), 700),
        send(Trigger::Commit(1), partial("eins zwei")),
        wait(Trigger::Commit(1), 700),
        send(Trigger::Commit(1), partial("eins zwei drei")),
        wait(Trigger::Commit(1), 700),
        send(Trigger::Commit(1), partial("eins zwei drei vier")),
        wait(Trigger::Commit(1), 700),
        send(
            Trigger::Commit(1),
            committed("eins zwei drei vier", 0.1, 2.0),
        ),
    ]));
    let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
    let mut handle = recogniser.start(request()).unwrap();
    let mut events = Vec::new();
    for frame in mulaw(5) {
        handle.offer_frame(frame).unwrap();
        events.extend(handle.by_ref());
    }
    wait_for(
        &mut handle,
        &mut events,
        "hypothesis eins",
        Duration::from_secs(3),
    );
    handle.end_input().unwrap();
    events.extend(handle.by_ref());
    let described: Vec<String> = events.iter().map(describe).collect();
    assert_eq!(
        handle.failure(),
        None,
        "the server sent a message at most 700 ms apart, never silent for the 2 s read timeout; \
         events {described:?}"
    );
    assert_eq!(
        described.last().map(String::as_str),
        Some("usage"),
        "{described:?}"
    );
    assert!(
        described
            .iter()
            .any(|event| event == "commit eins zwei drei vier"),
        "{described:?}"
    );
}

/// A server that accepts the handshake and then never says anything, not even
/// `session_started`, has transcribed nothing and answered no commit. The recognition must not
/// end as a success with a final usage record and no transcript: that reads, downstream, as a
/// caller who said nothing.
#[test]
fn a_server_that_never_says_anything_does_not_end_the_recognition_as_a_success() {
    let server = FixtureServer::start(Script::Steps(Vec::new()));
    let mut recogniser = ElevenLabsRecogniser::new(
        Endpoint::new()
            .with_url(&server.url)
            .with_read_timeout(Duration::from_millis(800))
            .with_connect_timeout(Duration::from_secs(5)),
        manual(InputFormat::Ulaw8000),
        Arc::new(FixedKey(KEY)),
        "elevenlabs-api-key",
    )
    .unwrap();
    let mut handle = recogniser.start(request()).unwrap();
    let mut events = Vec::new();
    for frame in mulaw(50) {
        handle.offer_frame(frame).unwrap();
        events.extend(handle.by_ref());
    }
    handle.end_input().unwrap();
    let ended = Instant::now();
    events.extend(handle.by_ref());
    let described: Vec<String> = events.iter().map(describe).collect();
    assert_eq!(
        server.seen()[0].commits,
        1,
        "the end-of-input commit reached the server"
    );
    assert!(
        handle.failure().is_some(),
        "one second of audio sent, its commit never answered and no message at all from the \
         server, yet no failure after {:?}; events {described:?}, usage {:?}",
        ended.elapsed(),
        usage_final(&events)
    );
    assert_eq!(
        usage_final(&events).map(|(_, is_final)| is_final),
        Some(false)
    );
}

/// With `vad`, a VAD commit still on its way when input ends answers for the binding's
/// end-of-input commit, and a transcript of the audio after it that arrives more than
/// `VAD_END_QUIET` (500 ms) later is dropped while the recognition ends as a success. The
/// protocol names no commit in a committed message, so the binding cannot tell the two answers
/// apart; `stt_protocol.rs` documents the loss. This case asserts that state as it is today: when
/// a later protocol revision pairs commits with their answers, the tail is committed, this case
/// goes red, and it is rewritten to expect `commit zwei` before `usage`.
#[test]
fn with_vad_a_tail_transcript_later_than_the_quiet_window_is_dropped_today() {
    let server = FixtureServer::start(Script::Steps(vec![
        send(Trigger::Open, session_started()),
        send(Trigger::Audio(5), partial("eins")),
        send(Trigger::Commit(1), committed("eins", 0.1, 0.4)),
        wait(Trigger::Commit(1), 700),
        send(Trigger::Commit(1), committed("zwei", 0.5, 0.8)),
    ]));
    let config = TranscriptionConfig::new(InputFormat::Ulaw8000)
        .with_commit_strategy(CommitStrategy::Vad(VadSettings::default()));
    let mut recogniser = server.recogniser(config);
    let mut handle = recogniser.start(request()).unwrap();
    let mut events = Vec::new();
    for frame in mulaw(40) {
        handle.offer_frame(frame).unwrap();
        events.extend(handle.by_ref());
    }
    handle.end_input().unwrap();
    events.extend(handle.by_ref());
    let described: Vec<String> = events.iter().map(describe).collect();
    assert_eq!(handle.failure(), None, "{described:?}");
    assert_eq!(
        described,
        ["hypothesis eins", "commit eins", "usage"],
        "story:elevenlabs-realtime-stt-adapter keeps the documented VAD tail loss; if the tail \
         is now committed, rewrite this case to expect it"
    );
}
