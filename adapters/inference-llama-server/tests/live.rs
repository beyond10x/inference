//! The live test against a running `llama-server`. It is ignored and needs the `live` feature:
//!
//! ```text
//! INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:8080/v1 \
//! INFERENCE_LLAMA_SERVER_MODEL=qwen2.5-1.5b-instruct \
//!     cargo test -p inference-llama-server --features live -- --ignored --nocapture
//! ```
//!
//! The crate documentation says how to start the server by hand. Run while either variable is
//! unset, the test returns at once and prints that it skipped. Against a server it asserts at
//! least two text deltas before the generation completes and a refused offer of input while it
//! is Active, and prints the server build, the usage reported and the wall time.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId};
use inference_core::model::{Adapter, Capability, Model};
use inference_llama_server::{Config, LlamaServer, declare_adapter, declare_model};
use inference_llm::llm_core::{Item, TurnRequest};
use inference_llm::{CapabilityNotClaimed, GenerationEvent, GenerationRequest, InputRefused};

const URL_VARIABLE: &str = "INFERENCE_LLAMA_SERVER_URL";
const MODEL_VARIABLE: &str = "INFERENCE_LLAMA_SERVER_MODEL";

fn variable(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The `build_info` of the server's `/props`, read over plain HTTP from the host and port of
/// `base_url`; what went wrong otherwise.
fn server_build(base_url: &str) -> String {
    let Some(authority) = base_url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
    else {
        return "unknown: the base URL is not plain HTTP".to_owned();
    };
    let read = || -> std::io::Result<String> {
        let mut socket = TcpStream::connect(authority)?;
        socket.set_read_timeout(Some(Duration::from_secs(10)))?;
        write!(
            socket,
            "GET /props HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
        )?;
        let mut response = String::new();
        socket.read_to_string(&mut response)?;
        Ok(response)
    };
    match read() {
        Ok(response) => response
            .split_once("\r\n\r\n")
            .and_then(|(_, body)| serde_json::from_str::<serde_json::Value>(body).ok())
            .and_then(|props| props.get("build_info").cloned())
            .map_or_else(
                || "unknown: /props names no build_info".to_owned(),
                |build| build.to_string(),
            ),
        Err(error) => format!("unknown: {error}"),
    }
}

#[test]
#[ignore = "needs a running llama-server named by INFERENCE_LLAMA_SERVER_URL and INFERENCE_LLAMA_SERVER_MODEL"]
fn a_running_server_streams_a_generation_and_refuses_input_during_output() {
    let (Some(base_url), Some(served_model)) = (variable(URL_VARIABLE), variable(MODEL_VARIABLE))
    else {
        println!("skipped: set {URL_VARIABLE} and {MODEL_VARIABLE} to run against a llama-server");
        return;
    };
    let mut config = Config::new(base_url.clone(), served_model.clone());
    config.max_output_tokens = 64;
    let (model, _) = Model::declare(declare_model());
    let (adapter, _) =
        Adapter::declare(declare_adapter(model.model_id(), &config)).expect("one slot");
    let mut generator = LlamaServer::new(&config)
        .expect("the configured base URL makes a binding")
        .generator(adapter)
        .expect("the binding declares what the adapter claims");

    let began = Instant::now();
    let mut handle = generator
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn: TurnRequest::new(
                served_model,
                vec![Item::user(
                    "Count from one to ten in words, separated by commas.",
                )],
            ),
        })
        .expect("no generation runs yet");

    let mut deltas = Vec::new();
    let mut refusal = None;
    let mut first_delta = None;
    let mut completed = None;
    while let Some(event) = handle.next() {
        match event {
            GenerationEvent::TextDelta(delta) => {
                assert!(completed.is_none(), "a delta after completion");
                deltas.push(delta.text);
                if refusal.is_none() {
                    first_delta = Some(began.elapsed());
                    assert_eq!(
                        handle.output().generation().state(),
                        GenerationState::Active
                    );
                    refusal = Some(handle.offer_input("and now in French".to_owned()));
                }
            }
            GenerationEvent::Completed(_) => completed = Some(began.elapsed()),
            GenerationEvent::Cancelled(..) => {
                panic!("the generation was cancelled: {:?}", handle.failure())
            }
            GenerationEvent::Started(_) | GenerationEvent::SegmentCommitted(_) => {}
        }
    }

    let completed = completed.expect("the generation completed");
    assert!(
        deltas.len() >= 2,
        "at least two deltas before completion: {deltas:?}"
    );
    assert_eq!(
        refusal,
        Some(Err(InputRefused::CapabilityNotClaimed(
            CapabilityNotClaimed {
                capability: Capability::ConcurrentInputOutput,
            }
        ))),
        "input offered while the generation is Active is refused"
    );
    assert_eq!(handle.outcomes().len(), 1, "the refused input ran no turn");

    println!("server build: {}", server_build(&base_url));
    println!(
        "usage: {:?}",
        handle
            .outcomes()
            .first()
            .map(|outcome| &outcome.observation.usage)
    );
    println!(
        "deltas: {}, first after {first_delta:?}, completed after {completed:?}",
        deltas.len()
    );
    println!("text: {:?}", deltas.concat());
}
