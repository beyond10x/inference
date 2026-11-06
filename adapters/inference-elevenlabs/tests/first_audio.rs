//! First audio: the first frame of an utterance arrives while its segment is Queued, before the
//! utterance ends, at position 0 in the configured format, carrying the first chunk the server
//! sent; and what the binding sent to get it is the protocol's.

mod support;

use inference_elevenlabs::OutputFormat;
use inference_tts::inference_audio::AudioEncoding;
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use serde_json::{Value, json};
use support::{FixtureServer, Script, committed};

#[test]
fn the_first_frame_is_the_first_chunk_at_position_zero_in_the_configured_format() {
    let server = FixtureServer::start(OutputFormat::Ulaw8000, Script::Serve);
    let mut synthesiser = server.synthesiser(OutputFormat::Ulaw8000);
    let mut handle = synthesiser.start().unwrap();
    let segment = committed(&["Hello, this is a short test."]).remove(0);
    let id = segment.segment_id();
    handle.offer_segment(segment).unwrap();

    let Some(SynthesisEvent::Audio(first)) = handle.next() else {
        panic!("the first event is not audio: {:?}", handle.failure());
    };
    assert_eq!(first.segment_id(), id);
    assert_eq!(
        handle.output().segment(id).map(|segment| segment.state()),
        Some(SegmentState::Queued),
        "the frame streams before its segment completes"
    );
    assert!(!handle.output().utterance_ended());
    let frame = first.frame();
    assert_eq!(frame.format.encoding, AudioEncoding::PcmMulaw);
    assert_eq!(frame.format.sample_rate.get(), 8_000);
    assert_eq!(frame.format.channels.get(), 1);
    assert_eq!(frame.position.get(), 0);
    assert_eq!(frame.sample_count, 400);
    assert_eq!(
        frame.data,
        support::fixture_chunks(OutputFormat::Ulaw8000)[0],
        "the frame carries the chunk as sent"
    );

    let Some(SynthesisEvent::Audio(second)) = handle.next() else {
        panic!("the second event is not audio");
    };
    assert_eq!(second.frame().position.get(), 400, "positions follow on");
    assert_eq!(server.violations(), Vec::<String>::new());
}

#[test]
fn the_binding_sends_one_space_then_the_text_with_one_trailing_space_and_flush_then_the_end() {
    let server = FixtureServer::start(OutputFormat::Pcm24000, Script::Serve);
    let mut synthesiser = server.synthesiser(OutputFormat::Pcm24000);
    let mut handle = synthesiser.start().unwrap();
    for segment in committed(&["First sentence.  \n", "Second one."]) {
        handle.offer_segment(segment).unwrap();
    }
    handle.end_utterance().unwrap();
    let events: Vec<SynthesisEvent> = handle.by_ref().collect();
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert!(matches!(events.last(), Some(SynthesisEvent::Usage(_))));

    assert_eq!(
        server.next_client_close(std::time::Duration::from_secs(5)),
        Some(0)
    );
    let seen = server.seen();
    assert_eq!(
        seen.len(),
        1,
        "story:elevenlabs-multi-context-tts-connection: one connection per synthesis"
    );
    let parsed = |text: &str| serde_json::from_str::<Value>(text).unwrap();
    let messages: Vec<Value> = seen[0].messages.iter().map(|m| parsed(m)).collect();
    assert_eq!(
        messages,
        [
            json!({"text": " ", "context_id": "s0"}),
            json!({"text": "First sentence. ", "context_id": "s0", "flush": true}),
            json!({"context_id": "s0", "close_context": true}),
            json!({"text": " ", "context_id": "s1"}),
            json!({"text": "Second one. ", "context_id": "s1", "flush": true}),
            json!({"context_id": "s1", "close_context": true}),
            json!({"close_socket": true}),
        ],
        "story:elevenlabs-multi-context-tts-connection: each segment is a context of one connection"
    );
    assert!(seen[0].query.contains("model_id=eleven_flash_v2_5"));
    assert!(seen[0].query.contains("output_format=pcm_24000"));
    assert_eq!(server.violations(), Vec::<String>::new());
}

#[test]
fn a_segment_is_sent_before_the_utterance_ends() {
    let server = FixtureServer::start(OutputFormat::Alaw8000, Script::Serve);
    let mut synthesiser = server.synthesiser(OutputFormat::Alaw8000);
    let mut handle = synthesiser.start().unwrap();
    let segment = committed(&["Early words."]).remove(0);
    let id = segment.segment_id();
    handle.offer_segment(segment).unwrap();
    let events: Vec<SynthesisEvent> = handle.by_ref().collect();
    assert!(!handle.output().utterance_ended());
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SynthesisEvent::Audio(_)))
            .count(),
        4
    );
    assert!(matches!(events.last(), Some(SynthesisEvent::Completed(_))));
    assert_eq!(
        handle.output().segment(id).map(|segment| segment.state()),
        Some(SegmentState::Synthesized)
    );
    assert_eq!(server.seen().len(), 1);
}
