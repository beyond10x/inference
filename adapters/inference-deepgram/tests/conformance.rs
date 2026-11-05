//! The backend-neutral STT checks of `inference_testkit::stt::conformance`, every item, against
//! the binding over the local fixture server, which also holds every connection to the protocol.
//! The binding claims every item: it streams both ways, so the claims check observes both
//! capabilities, and its failures are typed, so the failure check names one.

mod support;

use std::time::Duration;

use inference_deepgram::{CloseCause, DeepgramError, DeepgramRecognition, claims};
use inference_stt::Recogniser;
use inference_stt::inference_core::model::Capability;
use inference_stt::inference_core::usage::{Quantity, QuantityKind};
use inference_testkit::stt::conformance::{self, Input, ObservedClaims};
use support::{FixtureServer, Script};

fn input() -> Input {
    Input::default()
        .with_patience(Duration::from_secs(3))
        .with_drain_bound(Duration::from_secs(8))
}

fn server() -> FixtureServer {
    FixtureServer::start(Script::fixture("conversation.jsonl"))
}

fn assert_clean(server: &FixtureServer) {
    assert_eq!(server.violations(), Vec::<String>::new());
    assert!(!server.seen().is_empty(), "the server saw no connection");
}

#[test]
fn a_committed_span_is_never_moved_again_and_an_external_revision_of_it_is_refused() {
    let server = server();
    let observed = conformance::committed_final(&mut server.recogniser(), &input()).unwrap();
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
    let observed = conformance::spans_in_order(&mut server.recogniser(), &input()).unwrap();
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
    let observed = conformance::end_of_input(&mut server.recogniser(), &input()).unwrap();
    assert_eq!(observed.committed, 2);
    assert!(observed.drained_in < Duration::from_secs(8));
    assert_eq!(server.seen()[0].close_streams, 1);
    assert_clean(&server);
}

#[test]
fn a_dropped_recognition_frees_its_slot_and_revives_nothing() {
    let server = server();
    let observed = conformance::cancellation(&mut server.recogniser(), &input()).unwrap();
    assert_eq!(observed.max_concurrency, 1);
    assert_eq!(
        server.next_client_close(Duration::from_secs(5)),
        Some(0),
        "the dropped recognition closed its connection"
    );
    assert_eq!(
        server.seen().len(),
        2,
        "the fresh one ran on a connection of its own"
    );
    assert_clean(&server);
}

#[test]
fn a_backend_that_closes_mid_stream_ends_the_recognition_with_a_typed_error() {
    let server = FixtureServer::start(Script::closes_after_first(1011, "NET-0000"));
    let mut recogniser = server.recogniser();
    let observed = conformance::backend_failure(&mut recogniser, &input(), |handle| {
        handle.failure().map(ToString::to_string)
    })
    .unwrap();
    assert!(observed.error.contains("1011"), "{}", observed.error);
    assert_eq!(observed.committed, 0, "the open span stays open");
    assert_clean(&server);
}

#[test]
fn the_failure_named_is_the_close_cause() {
    let server = FixtureServer::start(Script::closes_after_first(1011, "NET-0000"));
    let cause = |handle: &DeepgramRecognition| match handle.failure() {
        Some(DeepgramError::Closed { cause, .. }) => Some(format!("{cause:?}")),
        _ => None,
    };
    let observed = conformance::backend_failure(&mut server.recogniser(), &input(), cause).unwrap();
    assert_eq!(observed.error, format!("{:?}", CloseCause::ServerTimeout));
}

#[test]
fn a_completed_recognition_yields_one_final_usage_record_last_with_the_audio_offered() {
    let server = server();
    let observed = conformance::usage_recorded(&mut server.recogniser(), &input()).unwrap();
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
    let mut recogniser = server.recogniser();
    assert_eq!(recogniser.adapter().capabilities(), claims().as_slice());
    let observed = conformance::claims_observed(&mut recogniser, &input()).unwrap();
    assert_eq!(
        observed,
        ObservedClaims {
            incremental_input: true,
            output_streaming: true,
        }
    );
    assert_eq!(
        claims(),
        [Capability::IncrementalInput, Capability::OutputStreaming]
    );
    assert_clean(&server);
}
