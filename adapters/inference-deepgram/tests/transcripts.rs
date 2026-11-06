//! Transcripts: interim results become a hypothesis and its revisions, a final or end-of-turn
//! result commits the span, `UtteranceEnd` commits what `speech_final` did not, an already
//! finalized `UtteranceEnd` is ignored, empty transcripts emit nothing, and `CloseStream` then the
//! summary `Metadata` end the stream with the usage record last.

mod support;

use inference_stt::{RecognitionHandle, TranscriptEvent};
use support::{FixtureServer, Script, describe, recognise, tone};

fn heard(fixture: &str) -> (Vec<String>, FixtureServer) {
    let server = FixtureServer::start(Script::fixture(fixture));
    let (handle, events) = recognise(&mut server.recogniser(), tone(8));
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    let words = events.iter().map(describe).collect();
    assert_eq!(server.violations(), Vec::<String>::new());
    (words, server)
}

#[test]
fn interim_then_final_then_speech_final_is_a_hypothesis_a_revision_and_a_commit_in_order() {
    let (words, _server) = heard("conversation.jsonl");
    assert_eq!(
        words,
        [
            "hypothesis hello",
            "revision hello there",
            "commit",
            "hypothesis how are",
            "revision how are you",
            "commit",
            "usage",
        ]
    );
}

#[test]
fn an_utterance_end_without_speech_final_commits_the_open_span() {
    let (words, _server) = heard("utterance-end.jsonl");
    assert_eq!(
        words,
        [
            "hypothesis good morning",
            "commit",
            "hypothesis bye",
            "commit",
            "usage"
        ]
    );
}

#[test]
fn an_utterance_end_already_finalized_is_ignored() {
    let (words, _server) = heard("finalized.jsonl");
    assert_eq!(
        words,
        ["hypothesis wait", "revision wait for it", "commit", "usage"],
        "one span, revised across the ignored UtteranceEnd"
    );
}

#[test]
fn empty_transcripts_produce_no_event() {
    let (words, _server) = heard("empty.jsonl");
    assert_eq!(words, ["hypothesis hi", "commit", "usage"]);
}

#[test]
fn close_stream_then_metadata_ends_the_stream_with_a_final_usage_record_last() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let (mut handle, events) = recognise(&mut server.recogniser(), tone(8));
    let Some(TranscriptEvent::Usage(record)) = events.last().map(|event| &event.payload) else {
        panic!("the last event is not the usage record: {events:?}");
    };
    assert!(record.r#final);
    assert_eq!(handle.provider_duration(), Some(1.6));
    assert!(handle.output().usage_recorded());
    assert!(handle.next().is_none(), "the stream stays ended");
    let seen = &server.seen()[0];
    assert_eq!(seen.close_streams, 1);
    assert_eq!(seen.audio_messages, 8);
    assert_eq!(seen.audio_bytes, 8 * 320);
}
