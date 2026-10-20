//! The live tool test against a running `llama-server` started with `--jinja`. It is ignored and
//! needs the `live` and `tools` features:
//!
//! ```text
//! INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:8080/v1 \
//! INFERENCE_LLAMA_SERVER_MODEL=qwen2.5-1.5b-instruct \
//!     cargo test -p inference-llama-server --features live,tools --test live_tools -- --ignored --nocapture
//! ```
//!
//! Run while either variable is unset, the test returns at once and prints that it skipped.
//! Against a server it declares one read-only tool, asks a question that needs it, asserts a
//! structured proposal for that tool, proposes it on the generation, integrates a fixed result and
//! asserts the generation completes with text on the same generation id. It prints the server
//! build, the proposal and the answer.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{Adapter, Model};
use inference_llama_server::tools::proposal;
use inference_llama_server::{Config, LlamaServer, declare_adapter, declare_model};
use inference_llm::llm_core::{Item, TurnRequest};
use inference_llm::tools::{tool_name, tool_spec};
use inference_llm::{GenerationEvent, GenerationRequest};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{DeclareTool, SideEffectClass, Tool, ToolCall};
use serde_json::json;

const URL_VARIABLE: &str = "INFERENCE_LLAMA_SERVER_URL";
const MODEL_VARIABLE: &str = "INFERENCE_LLAMA_SERVER_MODEL";

const QUESTION: &str = "What is the local time in Berlin right now? Use the local_time tool.";

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
#[ignore = "needs a running llama-server started with --jinja, named by INFERENCE_LLAMA_SERVER_URL and INFERENCE_LLAMA_SERVER_MODEL"]
fn a_running_server_proposes_a_tool_call_and_completes_after_its_result_on_one_generation() {
    let (Some(base_url), Some(served_model)) = (variable(URL_VARIABLE), variable(MODEL_VARIABLE))
    else {
        println!("skipped: set {URL_VARIABLE} and {MODEL_VARIABLE} to run against a llama-server");
        return;
    };
    let mut config = Config::new(base_url.clone(), served_model.clone());
    config.max_output_tokens = 128;
    config.tools = true;
    let (model, _) = Model::declare(declare_model());
    let (adapter, _) =
        Adapter::declare(declare_adapter(model.model_id(), &config)).expect("one slot");
    let mut generator = LlamaServer::new(&config)
        .expect("the configured base URL makes a binding")
        .generator(adapter)
        .expect("the binding declares what the adapter claims");
    let (tool, _) = Tool::declare(DeclareTool {
        name: "local_time".to_owned(),
        schema: json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
            "additionalProperties": false
        }),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    });

    let began = Instant::now();
    let mut turn = TurnRequest::new(served_model, vec![Item::user(QUESTION)]);
    turn.tools = vec![tool_spec(&tool)];
    let mut handle = generator
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn,
        })
        .expect("no generation runs yet");

    let first: Vec<GenerationEvent> = handle.by_ref().collect();
    let Some(GenerationEvent::Started(started)) = first.first() else {
        panic!("the first event is the start: {first:?}")
    };
    let generation_id = started.generation_id;
    let call = handle.take_proposal().unwrap_or_else(|| {
        panic!(
            "the server proposed no structured call: {first:?}, failure {:?}",
            handle.failure()
        )
    });
    assert_eq!(call.name, tool_name(tool.tool_id()), "{call:?}");
    let proposed_after = began.elapsed();

    let (opened, _) = Turn::open(OpenTurn {
        run_id: RunId::generate(),
    });
    let (basis, _) = Span::emit(
        &opened,
        opened.run_id(),
        StreamId::generate(),
        1,
        QUESTION.to_owned(),
    )
    .expect("an Open turn admits a span");
    let (basis, _) = basis.commit().expect("a Hypothesis commits");
    let input = proposal(
        &call,
        handle.output().generation().generation_id(),
        basis.span_id(),
    )
    .expect("the call names the declared tool");
    let (proposed, _) = ToolCall::propose(
        &tool,
        handle.output().generation(),
        &basis,
        input.arguments.clone(),
    )
    .expect("an Active generation proposes on a committed span");
    assert_eq!(proposed.generation_id(), generation_id);
    assert_eq!(proposed.tool_id(), tool.tool_id());

    handle
        .integrate(&call.call_id, json!({"time": "14:05"}), false)
        .expect("the call is awaited");
    let rest: Vec<GenerationEvent> = handle.by_ref().collect();
    let answer: String = rest
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::TextDelta(delta) => {
                assert_eq!(delta.generation_id, generation_id);
                Some(delta.text.as_str())
            }
            _ => None,
        })
        .collect();
    let Some(GenerationEvent::Completed(completed)) = rest.last() else {
        panic!(
            "the generation completes: {rest:?}, failure {:?}",
            handle.failure()
        )
    };
    assert_eq!(completed.generation_id, generation_id);
    assert!(!answer.trim().is_empty(), "the answer has text");
    assert_eq!(handle.outcomes().len(), 2, "two turns on one generation");

    println!("server build: {}", server_build(&base_url));
    println!(
        "proposal: {} {} after {proposed_after:?}",
        call.name.as_str(),
        call.arguments
    );
    println!("tool call: {:?}", proposed.state());
    println!(
        "generation: {generation_id:?}, completed after {:?}",
        began.elapsed()
    );
    println!("answer: {answer:?}");
}
