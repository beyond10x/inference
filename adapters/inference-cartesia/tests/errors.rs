//! How a synthesis ends when the server or the credential does not cooperate: an `error`
//! message, a close mid-segment, silence, a context the binding never opened, a refused
//! handshake and a missing key each fail the synthesis with their own error, never a panic or a
//! hang; a final chunk with `done` true completes its segment without a `done` message.

mod support;

use std::sync::Arc;
use std::time::Duration;

use inference_cartesia::{
    CartesiaError, CartesiaSynthesiser, CredentialError, OutputFormat, ServerError, VoiceConfig,
};
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixtureServer, NoKey, Script, VOICE, committed, within};

const FORMAT: OutputFormat = OutputFormat::MULAW_8000;

/// Speaks one segment with `synthesiser` and returns its events and its failure.
fn run(mut synthesiser: CartesiaSynthesiser) -> (Vec<SynthesisEvent>, Option<CartesiaError>) {
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["Something goes wrong."]).remove(0))
        .unwrap();
    handle.end_utterance().unwrap();
    let events: Vec<SynthesisEvent> = handle.by_ref().collect();
    (events, handle.failure().cloned())
}

fn frames(events: &[SynthesisEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, SynthesisEvent::Audio(_)))
        .count()
}

#[test]
fn an_error_message_fails_the_synthesis_with_the_servers_error() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::ErrorAfterFirst);
        let (events, failure) = run(server.synthesiser(FORMAT));
        assert_eq!(frames(&events), 1);
        assert_eq!(
            failure,
            Some(CartesiaError::Server(ServerError {
                status: Some(429),
                code: Some("concurrency_limited".to_owned()),
                title: "Too many concurrent requests".to_owned(),
                message: "The fixture's limit.".to_owned(),
            }))
        );
    });
}

#[test]
fn a_close_mid_segment_fails_the_synthesis_with_the_close_code() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::CloseAfterFirst);
        let (events, failure) = run(server.synthesiser(FORMAT));
        assert_eq!(frames(&events), 1);
        assert!(
            matches!(
                failure,
                Some(CartesiaError::Closed {
                    code: Some(1011),
                    ..
                })
            ),
            "{failure:?}"
        );
    });
}

#[test]
fn silence_fails_the_synthesis_as_stalled_within_the_read_timeout() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::Silent);
        let synthesiser = CartesiaSynthesiser::new(
            server
                .endpoint()
                .with_read_timeout(Duration::from_millis(300)),
            VoiceConfig::new(VOICE, FORMAT),
            Arc::new(support::FixedKey(support::KEY)),
            "cartesia-api-key",
        )
        .unwrap();
        let (events, failure) = run(synthesiser);
        assert_eq!(frames(&events), 0);
        assert_eq!(
            failure,
            Some(CartesiaError::Stalled(Duration::from_millis(300)))
        );
    });
}

#[test]
fn a_chunk_of_a_context_never_opened_is_a_protocol_violation() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::ForeignAfterFirst);
        let (_, failure) = run(server.synthesiser(FORMAT));
        assert!(
            matches!(failure, Some(CartesiaError::Protocol(_))),
            "{failure:?}"
        );
    });
}

#[test]
fn a_refused_handshake_is_a_refusal_and_a_missing_key_connects_nowhere() {
    within(|| {
        let server = FixtureServer::refusing(FORMAT);
        let (_, failure) = run(server.synthesiser(FORMAT));
        match failure {
            Some(CartesiaError::Connect(error)) => assert!(error.is_refusal(), "{error:?}"),
            other => panic!("{other:?}"),
        }

        let server = FixtureServer::start(FORMAT, Script::Serve);
        let synthesiser = CartesiaSynthesiser::new(
            server.endpoint(),
            VoiceConfig::new(VOICE, FORMAT),
            Arc::new(NoKey),
            "cartesia-api-key",
        )
        .unwrap();
        let (_, failure) = run(synthesiser);
        assert_eq!(
            failure,
            Some(CartesiaError::Credential(CredentialError::Missing))
        );
        assert!(server.seen().is_empty(), "nothing connected without a key");
    });
}

#[test]
fn a_final_chunk_completes_its_segment_without_a_done_message() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::DoneInChunk);
        let mut synthesiser = server.synthesiser(FORMAT);
        let mut handle = synthesiser.start().unwrap();
        let segment = committed(&["Done in the chunk."]).remove(0);
        let id = segment.segment_id();
        handle.offer_segment(segment).unwrap();
        handle.end_utterance().unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(handle.failure().is_none(), "{:?}", handle.failure());
        assert_eq!(frames(&events), 4);
        assert_eq!(
            handle.output().segment(id).map(|s| s.state()),
            Some(SegmentState::Synthesized)
        );
        assert_eq!(server.violations(), Vec::<String>::new());
    });
}
