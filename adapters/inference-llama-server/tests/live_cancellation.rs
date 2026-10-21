//! The live cancellation test against a running `llama-server`. It is ignored and needs the
//! `live` feature:
//!
//! ```text
//! INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:8080/v1 \
//! INFERENCE_LLAMA_SERVER_MODEL=qwen2.5-1.5b-instruct \
//!     cargo test -p inference-llama-server --features live --test live_cancellation \
//!     -- --ignored --nocapture
//! ```
//!
//! Run while either variable is unset, each test returns at once and prints that it skipped.
//!
//! Against a server the first starts a long generation, cancels it after the first delta and
//! asserts `GenerationCancelled` with no later delta: delivery cancellation. What the cancelled
//! generation still delivers is the usage record of the turn it cut, one request: the counts
//! `llm` reported for it, or every token count unknown when it reported none, as a server that
//! counts only on the last chunk of its stream has reported nothing for a stream cut in the
//! middle; not final either way. The test asserts that and prints the counts. Then it reads the
//! slot state of the server (`GET /slots` at
//! the server root) until no slot is processing or 10 s pass, and prints which happened: whether
//! the server stopped computing. A server that does not expose its slot state is reported as
//! "computation cancellation not observed", not as a failure.
//!
//! The second starts a short generation, takes its first delta, waits until the turn has ended
//! on the server and handed its end over, and cancels the generation before taking the rest:
//! the cancel delivers no more text, and still delivers the usage record of that turn, the
//! counts the server reported.

#![cfg(feature = "live")]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId};
use inference_core::model::{Adapter, Model};
use inference_core::usage::{QuantityKind, QuantitySource, UsageRecorded};
use inference_llama_server::{Config, LlamaServer, declare_adapter, declare_model};
use inference_llm::llm_core::{Item, TurnRequest};
use inference_llm::{GenerationEvent, GenerationRequest, Generator};

const URL_VARIABLE: &str = "INFERENCE_LLAMA_SERVER_URL";
const MODEL_VARIABLE: &str = "INFERENCE_LLAMA_SERVER_MODEL";

/// How long the test reads the slot state after the cancel.
const SETTLE: Duration = Duration::from_secs(10);

fn variable(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Whether some slot of the server is processing, read from `/slots` over plain HTTP at the host
/// and port of `base_url`; why it could not be read otherwise.
fn processing(base_url: &str) -> Result<bool, String> {
    let authority = base_url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .ok_or("the base URL is not plain HTTP")?;
    let mut socket = TcpStream::connect(authority).map_err(|error| error.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    write!(
        socket,
        "GET /slots HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|error| error.to_string())?;
    let mut response = String::new();
    socket
        .read_to_string(&mut response)
        .map_err(|error| error.to_string())?;
    let (head, body) = response.split_once("\r\n\r\n").ok_or("no HTTP response")?;
    let status = head.lines().next().unwrap_or_default();
    if !status.contains(" 200 ") {
        return Err(format!("/slots answered `{status}`"));
    }
    let slots: Vec<serde_json::Value> =
        serde_json::from_str(body).map_err(|_| "/slots is not a list of slots".to_owned())?;
    let mut any = false;
    for slot in &slots {
        any |= slot
            .get("is_processing")
            .and_then(serde_json::Value::as_bool)
            .ok_or("a slot names no is_processing")?;
    }
    Ok(any)
}

#[test]
#[ignore = "needs a running llama-server named by INFERENCE_LLAMA_SERVER_URL and INFERENCE_LLAMA_SERVER_MODEL"]
fn a_running_server_cancelled_after_the_first_delta_delivers_no_more_text_and_the_cut_turns_record()
{
    let (Some(base_url), Some(served_model)) = (variable(URL_VARIABLE), variable(MODEL_VARIABLE))
    else {
        println!("skipped: set {URL_VARIABLE} and {MODEL_VARIABLE} to run against a llama-server");
        return;
    };
    let mut config = Config::new(base_url.clone(), served_model.clone());
    config.max_output_tokens = 1024;
    let (model, _) = Model::declare(declare_model());
    let (adapter, _) =
        Adapter::declare(declare_adapter(model.model_id(), &config)).expect("one slot");
    let server = Arc::new(LlamaServer::new(&config).expect("the configured base URL binds"));
    let mut generator = Generator::new(adapter, Arc::clone(&server) as _)
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
                    "Write a long story, at least eight hundred words, about a lighthouse keeper.",
                )],
            ),
        })
        .expect("no generation runs yet");

    let first = loop {
        match handle.next() {
            Some(GenerationEvent::TextDelta(delta)) => break delta,
            Some(GenerationEvent::Cancelled(..)) => {
                panic!("the generation was cancelled: {:?}", handle.failure())
            }
            Some(_) => {}
            None => panic!("the generation ended before a delta"),
        }
    };
    let first_after = began.elapsed();
    let before = processing(&base_url);

    let generation_id = handle.output().generation().generation_id();
    let cancelled = handle.cancel().expect("the generation is Active");
    let cancelled_at = Instant::now();
    assert_eq!(cancelled.generation_id, generation_id);
    assert_eq!(
        handle.output().generation().state(),
        GenerationState::Cancelled
    );
    let later: Vec<GenerationEvent> = handle.by_ref().collect();
    let [GenerationEvent::Usage(cut)] = later.as_slice() else {
        panic!("the record of the turn it cut and no text after the cancel: {later:?}")
    };
    assert!(!cut.r#final, "the cut turn's counts are not final: {cut:?}");
    let requests = cut
        .quantities
        .iter()
        .find(|quantity| quantity.kind() == QuantityKind::Requests)
        .and_then(|quantity| quantity.value());
    assert_eq!(requests, Some(1), "the cut call is counted: {cut:?}");
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    drop(handle);

    let stopped = loop {
        match processing(&base_url) {
            Ok(false) => break Ok(cancelled_at.elapsed()),
            Ok(true) if cancelled_at.elapsed() < SETTLE => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(true) => {
                break Err(format!(
                    "a slot still processes {SETTLE:?} after the cancel"
                ));
            }
            Err(why) => break Err(why),
        }
    };
    let turns = server.running_turns();

    println!(
        "first delta {:?} after the start: {:?}",
        first_after, first.text
    );
    println!("delivery cancelled: GenerationCancelled, no later text");
    println!("usage record of the cut turn: {:?}", cut.quantities);
    println!("slots processing before the cancel: {before:?}");
    match stopped {
        Ok(after) => {
            println!("computation cancelled: no slot processing {after:?} after the cancel")
        }
        Err(why) => println!("computation cancellation not observed: {why}"),
    }
    println!("turn threads still running: {turns}");
}

/// The value the server reported for `kind` in `recorded`; panics unless it reported one.
fn reported(recorded: &UsageRecorded, kind: QuantityKind) -> i64 {
    let quantity = recorded
        .quantities
        .iter()
        .find(|quantity| quantity.kind() == kind)
        .unwrap_or_else(|| panic!("a {kind:?} quantity: {recorded:?}"));
    assert_eq!(
        quantity.source(),
        QuantitySource::ProviderReported,
        "{kind:?}: {recorded:?}"
    );
    quantity.value().expect("a reported quantity has a value")
}

#[test]
#[ignore = "needs a running llama-server named by INFERENCE_LLAMA_SERVER_URL and INFERENCE_LLAMA_SERVER_MODEL"]
fn a_running_server_cancelled_before_its_ended_turn_was_taken_still_delivers_its_usage() {
    let (Some(base_url), Some(served_model)) = (variable(URL_VARIABLE), variable(MODEL_VARIABLE))
    else {
        println!("skipped: set {URL_VARIABLE} and {MODEL_VARIABLE} to run against a llama-server");
        return;
    };
    let mut config = Config::new(base_url, served_model.clone());
    config.max_output_tokens = 16;
    let (model, _) = Model::declare(declare_model());
    let (adapter, _) =
        Adapter::declare(declare_adapter(model.model_id(), &config)).expect("one slot");
    let server = Arc::new(LlamaServer::new(&config).expect("the configured base URL binds"));
    let mut generator = Generator::new(adapter, Arc::clone(&server) as _)
        .expect("the binding declares what the adapter claims");
    let mut handle = generator
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn: TurnRequest::new(
                served_model,
                vec![Item::user("Count from one to fifty in words.")],
            ),
        })
        .expect("no generation runs yet");

    loop {
        match handle.next() {
            Some(GenerationEvent::TextDelta(_)) => break,
            Some(GenerationEvent::Usage(recorded)) => {
                panic!("the turn's usage was taken before its first delta: {recorded:?}")
            }
            Some(GenerationEvent::Cancelled(..)) => {
                panic!("the generation was cancelled: {:?}", handle.failure())
            }
            Some(_) => {}
            None => panic!("the generation ended before a delta"),
        }
    }
    let deadline = Instant::now() + SETTLE;
    while server.running_turns() > 0 {
        assert!(
            Instant::now() < deadline,
            "the turn of {} output tokens at most did not end within {SETTLE:?}",
            config.max_output_tokens
        );
        thread::sleep(Duration::from_millis(20));
    }

    handle.cancel().expect("the generation is Active");
    let later: Vec<GenerationEvent> = handle.by_ref().collect();
    let [GenerationEvent::Usage(recorded)] = later.as_slice() else {
        panic!("the usage record of the ended turn and nothing else: {later:?}")
    };
    assert!(recorded.r#final, "the turn ended: {recorded:?}");
    let input = reported(recorded, QuantityKind::InputTextTokens);
    let output = reported(recorded, QuantityKind::OutputTextTokens);
    assert!(input > 0 && (1..=16).contains(&output), "{recorded:?}");
    println!(
        "usage record delivered after the cancel: {input} input and {output} output tokens, \
         final, upstream model {:?}",
        recorded.upstream_model
    );
}
