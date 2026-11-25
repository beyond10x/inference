//! Adversary cases, second pass, on the rule the agent binding's documentation states for a
//! signed URL: a model built with `ElevenLabsAgent::signed` gives its URL up to the first open
//! that gets past its configuration and its runtime, whatever happens next; only an open
//! refused for its configuration, or whose runtime could not be built, leaves the URL with the
//! model; every later open that gets past its configuration and its runtime fails as
//! `AgentError::SignedUrlSpent`.
//!
//! - An open whose request cannot be built from the URL connects to nothing, and has taken the
//!   URL all the same: the next open is refused as spent.
//! - An open cancelled before anything connected has taken the URL all the same.
//! - A later open of a spent model with a configuration the binding refuses is refused for the
//!   configuration, not as spent; one with a configuration the binding takes is refused as
//!   spent.
//!
//! Every token here is synthetic: no provider issued it.
#![cfg(feature = "duplex")]

#[path = "support/agent.rs"]
mod fixture;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fixture::{AGENT, AgentServer, Handshake, Script};
use inference_elevenlabs::ElevenLabsAgent;
use inference_elevenlabs::agent::{AgentEndpoint, AgentError, SignedUrl};
use inference_elevenlabs::inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_elevenlabs::inference_duplex::{
    NativeSpeechModel, OpenError, SessionConfig, SessionFailureKind, TurnDetection, TurnTuning,
    Unsupported,
};

fn config() -> SessionConfig {
    let pcm = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).unwrap(),
        channels: NonZeroU16::MIN,
    };
    SessionConfig {
        voice: String::new(),
        instructions: String::new(),
        input_format: pcm,
        output_format: pcm,
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

fn endpoint() -> AgentEndpoint {
    AgentEndpoint::new()
        .with_connect_timeout(Duration::from_millis(500))
        .with_metadata_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3))
}

fn signed(server: &AgentServer, token: &str) -> SignedUrl {
    SignedUrl::new(format!(
        "{}/v1/convai/conversation?agent_id={AGENT}&token={token}",
        server.url
    ))
    .expect("a signed URL the binding takes")
}

/// The open of a configuration the binding takes, with the URL spent: a `Rejected` failure,
/// `SignedUrlSpent` as its reason.
fn assert_spent(model: &mut ElevenLabsAgent, after: &str) {
    let refused = model.open(config());
    assert!(
        matches!(
            &refused,
            Err(OpenError::Failed(failure)) if failure.kind == SessionFailureKind::Rejected
        ),
        "{after}: the next open was not refused as spent: {:?}",
        refused.err()
    );
    assert_eq!(
        model.open_error(),
        Some(&AgentError::SignedUrlSpent),
        "{after}: the next open was not refused as spent"
    );
}

/// `SignedUrl::new` takes a URL whose query holds `<`, and no handshake request can be built
/// from it: the open fails with nothing connected. That open got past its configuration and its
/// runtime, so it took the URL out of the model: the next open is refused as spent, and
/// connects to nothing either.
#[test]
fn a_signed_model_whose_open_could_build_no_request_and_connected_to_nothing_has_spent_its_url() {
    let server = AgentServer::start(Script::new(Vec::new()));
    let mut model =
        ElevenLabsAgent::signed(signed(&server, "sYnThEtIc<ToKeN0011223344"), endpoint());

    model
        .open(config())
        .expect_err("no request can be built from the URL");
    let first = model.open_error().cloned();
    assert!(
        matches!(first, Some(AgentError::Connect { status: None, .. })),
        "{first:?}"
    );
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.connections(), 0, "the first open connected");

    assert_spent(&mut model, "an open that could build no request");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.connections(), 0, "the spent open connected");
}

/// A cancel that lands after the open began and before anything connected, as one before the
/// handshake was first polled does, ends the open as cancelled. That open got past its
/// configuration and its runtime, so it took the URL out of the model: the next open is refused
/// as spent. The cancel is raced from another thread, and this case asserts nothing when no
/// attempt of the 40 wins the race, that is, ends as cancelled with no new connection at the
/// server: it then prints that and passes.
#[test]
fn a_signed_model_whose_open_was_cancelled_before_anything_connected_has_spent_its_url() {
    let server = {
        let mut script = Script::new(Vec::new());
        script.handshake = Handshake::Stall;
        AgentServer::start(script)
    };
    for attempt in 1..=40 {
        let before = server.connections();
        let mut model =
            ElevenLabsAgent::signed(signed(&server, "sYnThEtIcToKeN5566778899"), endpoint());
        let stop = Arc::new(AtomicBool::new(false));
        let cancelling = {
            let stop = Arc::clone(&stop);
            let canceller = model.canceller();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    canceller.cancel();
                }
            })
        };
        model.open(config()).expect_err("the open is cancelled");
        stop.store(true, Ordering::Relaxed);
        cancelling.join().unwrap();
        if model.open_error() != Some(&AgentError::Cancelled) {
            continue;
        }
        std::thread::sleep(Duration::from_millis(50));
        if server.connections() != before {
            continue;
        }
        assert_spent(
            &mut model,
            &format!("attempt {attempt}, an open cancelled with nothing connected"),
        );
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            server.connections(),
            before,
            "attempt {attempt}: the spent open connected"
        );
        return;
    }
    eprintln!("no open was cancelled before anything connected in 40 attempts: nothing asserted");
}

/// A model whose URL is spent admits the configuration of a later open first, as any model
/// does: a configuration the binding refuses is refused for that, with no open error, and a
/// configuration the binding takes is refused as spent. Neither connects.
#[test]
fn a_later_open_of_a_spent_signed_model_is_refused_for_its_configuration_or_else_as_spent() {
    let server = AgentServer::start(Script::new(Vec::new()));
    let mut model =
        ElevenLabsAgent::signed(signed(&server, "sYnThEtIcToKeN9988776655"), endpoint());
    let session = model.open(config()).expect("the first open has the URL");

    let later = model.open(SessionConfig {
        auto_response: false,
        ..config()
    });
    assert_eq!(
        later.err(),
        Some(OpenError::Unsupported(Unsupported::ManualResponse)),
        "a later open with a configuration the binding refuses"
    );
    assert_eq!(
        model.open_error(),
        None,
        "an open refused for its configuration left an open error"
    );

    assert_spent(&mut model, "an open that connected");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.connections(), 1, "a later open connected");
    drop(session);
}
