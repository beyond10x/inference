//! The backend-neutral TTS checks of `inference_testkit::tts::conformance`, every exit item,
//! against the binding over the local fixture server, which also holds every connection to the
//! protocol.

mod support;

use inference_cartesia::{OutputFormat, claims};
use inference_testkit::tts::conformance::{self, Utterance};
use inference_tts::Synthesiser;
use inference_tts::inference_core::model::Capability;
use support::{FixtureServer, Script, within};

const FORMAT: OutputFormat = OutputFormat::MULAW_8000;

fn assert_clean(server: &FixtureServer) {
    assert_eq!(server.violations(), Vec::<String>::new());
    assert!(!server.seen().is_empty(), "the server saw no connection");
}

#[test]
fn a_tentative_segment_is_refused_and_the_committed_one_is_spoken() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::Serve);
        let observed =
            conformance::tentative_refused(&mut server.synthesiser(FORMAT), &Utterance::default())
                .unwrap();
        assert_eq!(observed.frames_spoken, 4);
        assert_clean(&server);
    });
}

#[test]
fn every_segment_reaches_played_only_on_the_sinks_acknowledgement() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::Serve);
        let observed = conformance::played_only_by_sink(
            &mut server.synthesiser(FORMAT),
            &Utterance::default(),
        )
        .unwrap();
        assert!(observed.played.iter().all(|&(_, frames)| frames == 4));
        assert_clean(&server);
    });
}

#[test]
fn the_claims_are_exactly_what_the_binding_is_observed_to_do() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::Serve);
        let mut synthesiser = server.synthesiser(FORMAT);
        assert_eq!(synthesiser.adapter().capabilities(), claims().as_slice());
        let observed =
            conformance::claims_observed(&mut synthesiser, &Utterance::default()).unwrap();
        assert!(observed.incremental_input, "audio arrived before the end");
        assert!(observed.output_streaming, "frames arrived while Queued");
        assert_eq!(
            claims(),
            [Capability::OutputStreaming, Capability::IncrementalInput]
        );
        assert_clean(&server);
    });
}

#[test]
fn every_frame_has_the_configured_format_its_length_and_a_contiguous_position() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::Serve);
        let mut synthesiser = server.synthesiser(FORMAT);
        let expected = synthesiser.format();
        let observed =
            conformance::audio_valid(&mut synthesiser, &Utterance::default(), Some(expected))
                .unwrap();
        assert_eq!(observed.frames, 8);
        assert_eq!(observed.samples, 3_200);
        assert_clean(&server);
    });
}

#[test]
fn a_discarded_segment_yields_nothing_more_and_a_dropped_handle_frees_its_slot() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::Serve);
        let observed =
            conformance::cancellation(&mut server.synthesiser(FORMAT), &Utterance::default())
                .unwrap();
        assert_eq!(
            observed.discarded.len(),
            2,
            "the streamed segment was discarded too"
        );
        assert_eq!(observed.max_concurrency, 1);
        assert_clean(&server);
        assert!(
            server.seen().iter().any(|seen| seen.cancels.len() == 1),
            "the segment discarded while it streamed was cancelled on the wire"
        );
    });
}

#[test]
fn a_completed_utterance_yields_one_final_usage_record_last() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::Serve);
        let observed =
            conformance::usage_recorded(&mut server.synthesiser(FORMAT), &Utterance::default())
                .unwrap();
        assert_eq!(observed.frames, 8);
        assert_eq!(observed.audio_ms, 400);
        assert_eq!(
            observed.characters,
            Utterance::default()
                .sentences()
                .iter()
                .map(|sentence| sentence.chars().count() as i64)
                .sum::<i64>()
        );
        assert_clean(&server);
    });
}

#[test]
fn the_segments_of_one_synthesis_share_one_connection_each_on_a_context_of_its_own() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::Serve);
        conformance::usage_recorded(&mut server.synthesiser(FORMAT), &Utterance::default())
            .unwrap();
        let seen = server.seen();
        assert_eq!(seen.len(), 1, "one connection for the synthesis");
        let contexts: Vec<&str> = seen[0]
            .requests
            .iter()
            .map(|request| request["context_id"].as_str().unwrap())
            .collect();
        assert_eq!(contexts.len(), 2);
        assert_ne!(contexts[0], contexts[1]);
        assert_clean(&server);
    });
}
