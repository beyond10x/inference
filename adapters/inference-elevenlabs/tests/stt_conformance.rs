//! The backend-neutral STT checks of `inference_testkit::stt::conformance`, every item, against
//! the realtime speech-to-text binding over the local fixture server, which also holds every
//! connection to the protocol. The recogniser takes the checks' 16 kHz PCM16 audio with VAD
//! commit: the server commits the first span on its own and the binding's commit at the end of
//! input commits the second.

mod stt_support;

use std::time::{Duration, Instant};

use inference_elevenlabs::{
    CommitStrategy, ElevenLabsError, ElevenLabsRecogniser, InputFormat, TranscriptionConfig,
    VadSettings, stt_claims,
};
use inference_stt::Recogniser;
use inference_stt::inference_core::model::Capability;
use inference_stt::inference_core::usage::{Quantity, QuantityKind};
use inference_testkit::stt::conformance::{self, Input, ObservedClaims};
use stt_support::{FixtureServer, Script};

fn input() -> Input {
    Input::default()
        .with_patience(Duration::from_secs(3))
        .with_drain_bound(Duration::from_secs(8))
}

fn server() -> FixtureServer {
    FixtureServer::start(Script::fixture("stt-conversation.jsonl"))
}

fn recogniser(server: &FixtureServer) -> ElevenLabsRecogniser {
    server.recogniser(
        TranscriptionConfig::new(InputFormat::Pcm16000)
            .with_commit_strategy(CommitStrategy::Vad(VadSettings::default())),
    )
}

fn assert_clean(server: &FixtureServer) {
    assert_eq!(server.violations(), Vec::<String>::new());
    assert!(!server.seen().is_empty(), "the server saw no connection");
}

#[test]
fn a_committed_span_is_never_moved_again_and_an_external_revision_of_it_is_refused() {
    let server = server();
    let observed = conformance::committed_final(&mut recogniser(&server), &input()).unwrap();
    let texts: Vec<&str> = observed
        .committed
        .iter()
        .map(|(_, text)| text.as_str())
        .collect();
    assert_eq!(texts, ["hello there", "how are you"]);
    assert_eq!(observed.revisions_refused, 2);
    assert_clean(&server);
}

#[test]
fn spans_are_contiguous_one_open_at_a_time_and_commit_in_order() {
    let server = server();
    let observed = conformance::spans_in_order(&mut recogniser(&server), &input()).unwrap();
    assert_eq!(observed.committed, 2);
    assert_eq!(observed.span_events, 6);
    assert_eq!(
        observed.last_sequence, 7,
        "six span events and the usage record"
    );
    assert_clean(&server);
}

#[test]
fn the_end_of_input_ends_the_stream_with_every_span_committed() {
    let server = server();
    let observed = conformance::end_of_input(&mut recogniser(&server), &input()).unwrap();
    assert_eq!(observed.committed, 2);
    assert!(observed.drained_in < Duration::from_secs(8));
    assert_eq!(server.seen()[0].commits, 1);
    assert_clean(&server);
}

#[test]
fn a_dropped_recognition_frees_its_slot_and_revives_nothing() {
    let server = server();
    let observed = conformance::cancellation(&mut recogniser(&server), &input()).unwrap();
    assert_eq!(observed.max_concurrency, 1);
    assert_eq!(observed.committed, 2);
    let dropped = 1;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut closed = Vec::new();
    while !closed.contains(&dropped) {
        let left = deadline.saturating_duration_since(Instant::now());
        match server.next_client_close(left) {
            Some(index) => closed.push(index),
            None => break,
        }
    }
    assert!(
        closed.contains(&dropped),
        "the dropped recognition, connection {dropped} after the baseline, closed its connection; \
         closes seen: {closed:?}"
    );
    assert_eq!(
        server.seen().len(),
        3,
        "the baseline, the dropped and the fresh recognition each ran on a connection of its own"
    );
    assert_clean(&server);
}

#[test]
fn a_backend_that_closes_mid_stream_ends_the_recognition_with_a_typed_error() {
    let server = FixtureServer::start(Script::closes_after_first(1011, "internal"));
    let observed = conformance::backend_failure(&mut recogniser(&server), &input(), |handle| {
        handle.failure().map(ToString::to_string)
    })
    .unwrap();
    assert!(observed.error.contains("1011"), "{}", observed.error);
    assert_eq!(observed.committed, 0, "the open span stays open");
    assert_clean(&server);
}

#[test]
fn a_backend_that_answers_with_an_error_type_ends_the_recognition_with_it() {
    let server = FixtureServer::start(Script::sends_after_first(
        r#"{"message_type":"transcriber_error","error":"model unavailable"}"#,
    ));
    let named = |handle: &inference_elevenlabs::ElevenLabsRecognition| match handle.failure() {
        Some(ElevenLabsError::TranscriberError(message)) => Some(message.clone()),
        _ => None,
    };
    let observed = conformance::backend_failure(&mut recogniser(&server), &input(), named).unwrap();
    assert_eq!(observed.error, "model unavailable");
}

#[test]
fn a_completed_recognition_yields_one_final_usage_record_last_with_the_audio_offered() {
    let server = server();
    let observed = conformance::usage_recorded(&mut recogniser(&server), &input()).unwrap();
    assert_eq!(observed.audio_ms, 1_600);
    assert_eq!(
        observed.local_compute,
        Quantity::unknown(QuantityKind::LocalComputeMs),
        "nothing is computed here"
    );
    assert_clean(&server);
}

#[test]
fn the_claims_are_exactly_what_the_binding_is_observed_to_do() {
    let server = server();
    let mut recogniser = recogniser(&server);
    assert_eq!(recogniser.adapter().capabilities(), stt_claims().as_slice());
    let observed = conformance::claims_observed(&mut recogniser, &input()).unwrap();
    assert_eq!(
        observed,
        ObservedClaims {
            incremental_input: true,
            output_streaming: true,
        }
    );
    assert_eq!(
        stt_claims(),
        [Capability::IncrementalInput, Capability::OutputStreaming]
    );
    assert_clean(&server);
}
