//! Adversary cases on the one-session signed URL of the agent binding, each driving the binding
//! from the rule its documentation states: a model built with `ElevenLabsAgent::signed` gives
//! its URL up to the first open that dials, whatever happens next, and only an open refused
//! before it dials leaves the URL with the model.
//!
//! - An open the agent refuses for its formats has dialled: `ElevenLabsAgent::open_error` names
//!   what the agent reported, and the open with the corrected configuration fails as
//!   `AgentError::SignedUrlSpent`.
//! - An open that met a closed port has dialled: the next open fails as
//!   `AgentError::SignedUrlSpent` and connects to nothing, though a server listens there by then.
//!
//! Every token here is synthetic: no provider issued it.
#![cfg(feature = "duplex")]

#[path = "support/agent.rs"]
mod fixture;

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use fixture::{AGENT, AgentServer, Script};
use inference_elevenlabs::ElevenLabsAgent;
use inference_elevenlabs::agent::{self, AgentEndpoint, AgentError, SignedUrl};
use inference_elevenlabs::inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_elevenlabs::inference_duplex::{
    NativeSpeechModel, OpenError, SessionConfig, SessionFailureKind, TurnDetection, TurnTuning,
    Unsupported,
};

const TOKEN: &str = "sYnThEtIcToKeN1122334455";

fn pcm(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        voice: String::new(),
        instructions: String::new(),
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

fn endpoint() -> AgentEndpoint {
    AgentEndpoint::new()
        .with_connect_timeout(Duration::from_secs(3))
        .with_metadata_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3))
}

fn signed(base: &str) -> SignedUrl {
    SignedUrl::new(format!(
        "{base}/v1/convai/conversation?agent_id={AGENT}&token={TOKEN}"
    ))
    .expect("a signed URL the binding takes")
}

/// A server whose agent takes `pcm_16000` and gives `pcm_24000`, on every connection.
fn agent_giving_24k() -> AgentServer {
    let mut script = Script::new(Vec::new());
    script.metadata = Some(("pcm_16000", "pcm_24000"));
    AgentServer::start(script)
}

/// Opens `model` expecting `pcm_16000` both ways, which the agent of [`agent_giving_24k`]
/// refuses, and returns the configuration corrected from what `open_error` named.
fn refused_for_its_formats(model: &mut ElevenLabsAgent) -> SessionConfig {
    let first = model.open(config());
    assert_eq!(
        first.err(),
        Some(OpenError::Unsupported(Unsupported::OutputFormat(pcm(
            16_000
        )))),
        "the first open is refused for the output format of its configuration"
    );
    let (input, output) = match model.open_error() {
        Some(AgentError::FormatMismatch { input, output }) => (input.clone(), output.clone()),
        other => panic!("the open error names no formats: {other:?}"),
    };
    SessionConfig {
        input_format: agent::audio_format(&input).expect("a format the binding maps"),
        output_format: agent::audio_format(&output).expect("a format the binding maps"),
        ..config()
    }
}

/// The control: on a public agent's model the corrected configuration opens, so the fixture and
/// the corrected configuration are sound.
#[test]
fn a_public_model_refused_for_its_formats_opens_with_the_formats_the_error_named() {
    let server = agent_giving_24k();
    let mut model =
        ElevenLabsAgent::public(AGENT, endpoint().with_url(&server.url)).expect("a valid agent id");
    let corrected = refused_for_its_formats(&mut model);
    let second = model.open(corrected);
    assert!(
        second.is_ok(),
        "{:?} / {:?}",
        second.err(),
        model.open_error()
    );
}

/// The open with the URL spent: a `Rejected` failure, `SignedUrlSpent` as its reason.
fn assert_spent(model: &mut ElevenLabsAgent, config: SessionConfig) {
    let second = model.open(config);
    assert!(
        matches!(
            &second,
            Err(OpenError::Failed(failure)) if failure.kind == SessionFailureKind::Rejected
        ),
        "the open after one that dialled was not refused as spent: {second:?}"
    );
    assert_eq!(model.open_error(), Some(&AgentError::SignedUrlSpent));
}

/// An open the agent refuses for its formats has dialled, so it spends the URL: the error names
/// the agent's formats, and the corrected open needs a model built with a new signed URL.
#[test]
fn a_signed_model_refused_for_the_agents_formats_has_spent_its_url_for_the_corrected_open() {
    let server = agent_giving_24k();
    let mut model = ElevenLabsAgent::signed(signed(&server.url), endpoint());
    let corrected = refused_for_its_formats(&mut model);
    assert_eq!(
        model.open_error(),
        Some(&AgentError::FormatMismatch {
            input: "pcm_16000".to_owned(),
            output: "pcm_24000".to_owned(),
        }),
        "the open error still names the agent's formats after the refusal"
    );
    assert_eq!(corrected.output_format, pcm(24_000));
    assert_spent(&mut model, corrected);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.connections(), 1, "the spent open connected");
}

/// An open that met a closed port has dialled, so it spends the URL, though it connected to
/// nothing: the next open is refused and connects to no server that listens there by then.
#[test]
fn a_signed_model_whose_open_met_a_closed_port_has_spent_its_url_and_dials_no_later_listener() {
    let address = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.local_addr().expect("an address")
    };
    let mut model = ElevenLabsAgent::signed(signed(&format!("ws://{address}")), endpoint());
    model.open(config()).expect_err("nothing listens there");
    assert!(
        matches!(
            model.open_error(),
            Some(AgentError::Connect { status: None, .. })
        ),
        "{:?}",
        model.open_error()
    );

    let listener = std::net::TcpListener::bind(address).expect("the port is free again");
    listener.set_nonblocking(true).expect("non-blocking");
    assert_spent(&mut model, config());
    std::thread::sleep(Duration::from_millis(50));
    let accepted = listener.accept();
    assert!(
        matches!(&accepted, Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "the spent open connected to the server that listens now: {accepted:?}"
    );
}
