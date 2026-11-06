//! The backend-neutral TTS checks of `inference_testkit::tts::conformance`, every exit item,
//! against the OpenAI speech binding over the local fixture server, which also holds every
//! request to the protocol. The frames asked for are μ-law at 8 kHz, so every check also runs
//! through the resampler and the G.711 encoder.

mod speech_support;

use inference_openai::speech::claims;
use inference_testkit::tts::conformance::{self, Utterance};
use inference_tts::Synthesiser;
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::inference_core::model::Capability;
use speech_support::{FixtureServer, Script, format, within_deadline};

fn mulaw_8k() -> AudioFormat {
    format(AudioEncoding::PcmMulaw, 8_000)
}

fn assert_clean(server: &FixtureServer) {
    assert_eq!(server.violations(), Vec::<String>::new());
    assert!(!server.seen().is_empty(), "the server saw no request");
}

#[test]
fn a_tentative_segment_is_refused_and_the_committed_one_is_spoken() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let observed = conformance::tentative_refused(
            &mut server.synthesiser(mulaw_8k()),
            &Utterance::default(),
        )
        .unwrap();
        assert!(observed.frames_spoken > 0);
        assert_clean(&server);
    });
}

#[test]
fn every_segment_reaches_played_only_on_the_sinks_acknowledgement() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let observed = conformance::played_only_by_sink(
            &mut server.synthesiser(mulaw_8k()),
            &Utterance::default(),
        )
        .unwrap();
        assert_eq!(observed.played.len(), 2);
        assert!(observed.played.iter().all(|&(_, frames)| frames > 0));
        assert_clean(&server);
    });
}

#[test]
fn the_claims_are_exactly_what_the_binding_is_observed_to_do() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let mut synthesiser = server.synthesiser(mulaw_8k());
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
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let mut synthesiser = server.synthesiser(mulaw_8k());
        assert_eq!(synthesiser.format(), mulaw_8k());
        let observed =
            conformance::audio_valid(&mut synthesiser, &Utterance::default(), Some(mulaw_8k()))
                .unwrap();
        assert_eq!(observed.format, mulaw_8k());
        assert_eq!(
            observed.samples, 3_200,
            "200 ms of 24 kHz tone per segment is 1600 samples at 8 kHz"
        );
        assert_clean(&server);
    });
}

#[test]
fn a_discarded_segment_yields_nothing_more_and_a_dropped_handle_frees_its_slot() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let observed =
            conformance::cancellation(&mut server.synthesiser(mulaw_8k()), &Utterance::default())
                .unwrap();
        assert_eq!(
            observed.discarded.len(),
            2,
            "the streamed segment was discarded too"
        );
        assert_eq!(observed.max_concurrency, 1);
        assert_eq!(server.violations(), Vec::<String>::new());
    });
}

#[test]
fn a_completed_utterance_yields_one_final_usage_record_last() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let observed =
            conformance::usage_recorded(&mut server.synthesiser(mulaw_8k()), &Utterance::default())
                .unwrap();
        assert_eq!(observed.audio_ms, 400);
        assert_eq!(
            observed.characters,
            Utterance::default()
                .sentences()
                .iter()
                .map(|sentence| sentence.chars().count() as i64)
                .sum::<i64>()
        );
        assert_eq!(server.inputs(), Utterance::default().sentences());
        assert_clean(&server);
    });
}
