//! Tool proposals of the adapter with its `tools` option on, against the scripted loopback
//! fixture of `common::scripted`, which replays streams recorded from a running `llama-server`
//! started with `--jinja` (see `fixtures/llama-server-b11752-tool.txt`). No server is started and
//! no weights are read.
//!
//! The `tool_calling` evidence of the capability record of the adapter with tools on,
//! `capability/reference-machine-cpu-tools.json`, names the tests of this file.

mod common;

use std::path::Path;

use common::SERVED_MODEL;
use common::scripted::{Scripted, TOOL_CALL, TOOL_NAMELESS, TOOL_RESULT};
use inference_core::generation::GenerationState;
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, Capability, Model};
use inference_llama_server::tools::{UndeclaredTool, proposal};
use inference_llama_server::{Config, LlamaServer, binding, declare_adapter, declare_model};
use inference_llm::llm_core::{CallId, ErrorCode, Item, ToolName, TurnRequest};
use inference_llm::tools::{tool_name, tool_spec};
use inference_llm::{GenerationEvent, GenerationHandle, GenerationRequest, Generator};
use inference_testkit::model::capability::{self, RecordRefused};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    DeclareTool, ProposeToolCall, SideEffectClass, Tool, ToolCall, ToolCallState, ToolId,
};
use serde_json::{Value, json};

/// The user's message of the recorded request.
const PROMPT: &str = "Say \"Let me check.\" and then use the local_time tool for Berlin.";

/// The name the recorded request offered the tool under: its identifier.
const TOOL_NAME: &str = "6f0c8a52-3b1d-4e7a-9c2f-5d8e1a4b7c30";

/// The identifier of the call the recorded server proposed.
const CALL_ID: &str = "drCuySC72O9i0Aup52hZORaYoK2S3CMa";

/// The prose the recorded server streamed before the call, one delta each.
const PROSE: [&str; 4] = ["Let", " me", " check", ".\n"];

/// The text the recorded server streamed after the result.
const ANSWER: &str = "The current time in Berlin is 14:05.";

/// The record committed for the adapter with tools on, on the reference machine, CPU only, the
/// server started with `--jinja`.
const RECORD: &str = "capability/reference-machine-cpu-tools.json";

/// The record committed for the default adapter, tools off, on the same machine.
const DEFAULT_RECORD: &str = "capability/reference-machine-cpu.json";

fn tool_id() -> ToolId {
    serde_json::from_value(json!(TOOL_NAME)).expect("a hyphenated identifier")
}

/// The read-only tool of the recorded request, declared under its identifier.
fn local_time() -> Tool {
    Tool::declare_as(
        tool_id(),
        DeclareTool {
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
        },
    )
    .0
}

fn config(base_url: &str) -> Config {
    let mut config = Config::new(base_url, SERVED_MODEL);
    config.tools = true;
    config
}

fn declared(config: &Config) -> Adapter {
    let (model, _) = Model::declare(declare_model());
    Adapter::declare(declare_adapter(model.model_id(), config))
        .expect("one slot is positive")
        .0
}

fn generator(fixture: &Scripted) -> Generator {
    let config = config(fixture.base_url());
    LlamaServer::new(&config)
        .expect("the fixture's base URL makes a binding")
        .generator(declared(&config))
        .expect("the binding declares what the adapter claims")
}

/// Starts the generation of the recorded request: the user's message and the tool.
fn start(generator: &mut Generator) -> GenerationHandle {
    let mut turn = TurnRequest::new(SERVED_MODEL, vec![Item::user(PROMPT)]);
    turn.tools = vec![tool_spec(&local_time())];
    generator
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn,
        })
        .expect("no generation runs yet")
}

fn deltas(events: &[GenerationEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::TextDelta(delta) => Some(delta.text.as_str()),
            _ => None,
        })
        .collect()
}

/// A committed span of a turn of its own: what the proposal rests on.
fn basis() -> Span {
    let (turn, _) = Turn::open(OpenTurn {
        run_id: RunId::generate(),
    });
    let (span, _) = Span::emit(
        &turn,
        turn.run_id(),
        StreamId::generate(),
        1,
        PROMPT.to_owned(),
    )
    .expect("an Open turn admits a span");
    span.commit().expect("a Hypothesis commits").0
}

/// Every piece of the recorded call: its name, its identifier, the tool's own name and each part
/// of its arguments.
const CALL_TEXT: [&str; 7] = [TOOL_NAME, CALL_ID, "local_time", "{", "}", "city", "Berlin"];

/// The pieces of the recorded call that no prose of the generation may carry, even an answer that
/// names the city after the result: the call's name, its identifier, the tool's own name and the
/// arguments' structure.
const CALL_STRUCTURE: [&str; 6] = [TOOL_NAME, CALL_ID, "local_time", "{", "}", "\"city\""];

/// Asserts that no segment of `handle` and no delta of `events` carries any of `call_text`.
fn no_call_text_in_any_segment(
    handle: &GenerationHandle,
    events: &[GenerationEvent],
    call_text: &[&str],
) {
    let texts: Vec<&str> = handle
        .output()
        .segments()
        .iter()
        .map(|segment| segment.text())
        .chain(deltas(events))
        .collect();
    assert!(!texts.is_empty(), "the generation has segments");
    for text in texts {
        for &call_text in call_text {
            assert!(
                !text.contains(call_text),
                "the segment {text:?} carries {call_text:?} of the call"
            );
        }
    }
}

#[test]
fn prose_then_a_call_in_one_turn_arrive_as_segments_and_one_structured_proposal() {
    let fixture = Scripted::new(vec![TOOL_CALL]);
    let mut generator = generator(&fixture);
    let mut handle = start(&mut generator);

    let events: Vec<GenerationEvent> = handle.by_ref().collect();
    assert_eq!(deltas(&events), PROSE, "{events:?}");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, GenerationEvent::Completed(_))),
        "the generation awaits the result: {events:?}"
    );
    assert_eq!(
        handle.output().generation().state(),
        GenerationState::Active
    );
    assert!(handle.awaits_results());

    let call = handle
        .take_proposal()
        .expect("the turn ended with one structured call");
    assert_eq!(call.call_id, CallId::new(CALL_ID).expect("a call id"));
    assert_eq!(call.name, tool_name(tool_id()));
    assert_eq!(call.arguments, json!({"city": "Berlin"}));
    assert_eq!(handle.take_proposal(), None, "one call in the round");
    no_call_text_in_any_segment(&handle, &events, &CALL_TEXT);

    let requests = fixture.requests();
    assert_eq!(requests.len(), 1);
    let tools = &requests[0].json()["tools"];
    assert_eq!(tools[0]["function"]["name"], json!(TOOL_NAME), "{tools}");
    assert_eq!(
        tools[0]["function"]["description"],
        json!("local_time"),
        "{tools}"
    );
}

#[test]
fn the_call_is_proposed_through_propose_tool_call_on_the_generation_that_made_it() {
    let fixture = Scripted::new(vec![TOOL_CALL]);
    let mut generator = generator(&fixture);
    let mut handle = start(&mut generator);
    let events: Vec<GenerationEvent> = handle.by_ref().collect();
    let Some(GenerationEvent::Started(started)) = events.first() else {
        panic!("the first event is the start: {events:?}")
    };
    let call = handle.take_proposal().expect("one call");

    let generation = handle.output().generation();
    let basis = basis();
    let input = proposal(&call, generation.generation_id(), basis.span_id())
        .expect("the call names the declared tool");
    assert_eq!(
        input,
        ProposeToolCall {
            tool_id: tool_id(),
            generation_id: started.generation_id,
            basis_span_id: basis.span_id(),
            arguments: json!({"city": "Berlin"}),
        }
    );

    let tool = local_time();
    assert_eq!(input.tool_id, tool.tool_id());
    let (proposed, event) = ToolCall::propose(&tool, generation, &basis, input.arguments.clone())
        .expect("the generation is Active and the basis committed");
    assert_eq!(proposed.generation_id(), started.generation_id);
    assert_eq!(proposed.generation_id(), generation.generation_id());
    assert_eq!(event.generation_id, started.generation_id);
    assert_eq!(proposed.tool_id(), tool_id());
    assert_eq!(event.tool_id, tool_id());
    assert_eq!(proposed.arguments(), &json!({"city": "Berlin"}));
    assert_eq!(proposed.state(), ToolCallState::Proposed);
    assert_eq!(proposed.side_effect(), SideEffectClass::ReadOnly);
}

#[test]
fn a_call_naming_no_declared_tool_maps_to_no_proposal() {
    let call = inference_llm::llm_core::ToolCall {
        call_id: CallId::new(CALL_ID).expect("a call id"),
        name: ToolName::new("local_time").expect("a tool name"),
        arguments: json!({"city": "Berlin"}),
    };
    let name = call.name.clone();
    assert_eq!(
        proposal(&call, GenerationId::generate(), basis().span_id()),
        Err(UndeclaredTool { name })
    );
}

#[test]
fn a_result_integrated_on_the_handle_runs_the_next_turn_on_the_same_generation() {
    let fixture = Scripted::new(vec![TOOL_CALL, TOOL_RESULT]);
    let mut generator = generator(&fixture);
    let mut handle = start(&mut generator);
    let first: Vec<GenerationEvent> = handle.by_ref().collect();
    let Some(GenerationEvent::Started(started)) = first.first() else {
        panic!("the first event is the start: {first:?}")
    };
    let generation_id = started.generation_id;
    let call = handle.take_proposal().expect("one call");
    no_call_text_in_any_segment(&handle, &first, &CALL_TEXT);

    handle
        .integrate(&call.call_id, json!({"time": "14:05"}), false)
        .expect("the call is awaited");
    let rest: Vec<GenerationEvent> = handle.by_ref().collect();
    assert_eq!(deltas(&rest).concat(), ANSWER, "{rest:?}");
    let Some(GenerationEvent::Completed(completed)) = rest.last() else {
        panic!("the generation completes: {rest:?}")
    };
    assert_eq!(completed.generation_id, generation_id);
    for event in first.iter().chain(&rest) {
        if let GenerationEvent::TextDelta(delta) = event {
            assert_eq!(delta.generation_id, generation_id, "{delta:?}");
        }
    }
    assert!(
        handle
            .output()
            .segments()
            .iter()
            .all(|segment| segment.generation_id() == generation_id)
    );
    assert_eq!(handle.outcomes().len(), 2, "two turns ran");
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    no_call_text_in_any_segment(&handle, &rest, &CALL_STRUCTURE);

    let requests = fixture.requests();
    assert_eq!(requests.len(), 2, "one request per turn");
    let messages = requests[1].json()["messages"].clone();
    assert_eq!(
        messages,
        json!([
            {"role": "user", "content": PROMPT},
            {
                "role": "assistant",
                "content": PROSE.concat(),
                "tool_calls": [{
                    "id": CALL_ID,
                    "type": "function",
                    "function": {"name": TOOL_NAME, "arguments": "{\"city\":\"Berlin\"}"}
                }]
            },
            {"role": "tool", "tool_call_id": CALL_ID, "content": "{\"time\":\"14:05\"}"}
        ]),
        "{messages}"
    );
    assert_eq!(
        messages[2]["tool_call_id"],
        Value::String(call.call_id.as_str().to_owned())
    );
}

#[test]
fn a_call_with_no_name_in_any_fragment_cancels_the_generation_and_proposes_nothing() {
    let fixture = Scripted::new(vec![TOOL_NAMELESS]);
    let mut generator = generator(&fixture);
    let mut handle = start(&mut generator);

    let events: Vec<GenerationEvent> = handle.by_ref().collect();
    assert!(
        matches!(events.last(), Some(GenerationEvent::Cancelled(..))),
        "{events:?}"
    );
    assert_eq!(
        handle.output().generation().state(),
        GenerationState::Cancelled
    );
    let failure = handle.failure().expect("the llm error is kept");
    assert_eq!(failure.code, ErrorCode::Protocol, "{failure:?}");
    assert!(
        failure.message.contains("no name"),
        "the refusal says the call has no name: {failure:?}"
    );
    assert_eq!(handle.take_proposal(), None);
    assert!(!handle.awaits_results());
    assert!(
        handle.outcomes().is_empty(),
        "no turn ended with an outcome"
    );
    assert_eq!(fixture.requests().len(), 1);
}

#[test]
fn with_tools_on_the_adapter_claims_output_streaming_and_tool_calling_and_the_binding_declares_them()
 {
    let on = config("http://127.0.0.1:8080/v1");
    let adapter = declared(&on);
    assert_eq!(
        adapter.capabilities(),
        [Capability::OutputStreaming, Capability::ToolCalling]
    );
    let declared_on = binding(&on).expect("a binding").capabilities().clone();
    assert!(declared_on.tools);
    assert!(declared_on.tool_choice);
    assert!(declared_on.output_streaming);
    assert!(!declared_on.input_during_output);
    let generator = LlamaServer::new(&on)
        .expect("a binding")
        .generator(adapter)
        .expect("the binding declares both claims");
    assert_eq!(
        generator.adapter().capabilities(),
        [Capability::OutputStreaming, Capability::ToolCalling]
    );

    let off = Config::new("http://127.0.0.1:8080/v1", SERVED_MODEL);
    assert!(!off.tools, "off by default");
    assert_eq!(
        declared(&off).capabilities(),
        [Capability::OutputStreaming],
        "off, the adapter record is unchanged"
    );
    let declared_off = binding(&off).expect("a binding").capabilities().clone();
    assert!(!declared_off.tools);
    assert!(!declared_off.tool_choice);
    let refused = LlamaServer::new(&off)
        .expect("a binding")
        .generator(declared(&on));
    assert!(
        refused.is_err(),
        "a tool_calling claim over a binding without tools is refused"
    );
}

#[test]
fn the_capability_record_evidences_tool_calling_with_the_tests_of_this_file() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let on = declared(&config("http://127.0.0.1:8080/v1"));
    let record =
        capability::load(crate_dir.join(RECORD), &on).unwrap_or_else(|refused| panic!("{refused}"));
    let tests: Vec<&str> = record
        .evidence
        .iter()
        .filter(|evidence| evidence.capability == Capability::ToolCalling)
        .flat_map(|evidence| evidence.tests.iter().map(String::as_str))
        .collect();
    for name in [
        "tool_proposals::prose_then_a_call_in_one_turn_arrive_as_segments_and_one_structured_proposal",
        "tool_proposals::the_call_is_proposed_through_propose_tool_call_on_the_generation_that_made_it",
        "tool_proposals::a_result_integrated_on_the_handle_runs_the_next_turn_on_the_same_generation",
        "tool_proposals::a_call_with_no_name_in_any_fragment_cancels_the_generation_and_proposes_nothing",
    ] {
        assert!(tests.contains(&name), "{name} is not evidence: {tests:?}");
    }

    let off = declared(&Config::new("http://127.0.0.1:8080/v1", SERVED_MODEL));
    assert_eq!(
        capability::load(crate_dir.join(RECORD), &off).map(drop),
        Err(RecordRefused::Unclaimed(Capability::ToolCalling)),
        "the record is the record of the adapter with tools on"
    );
    assert_eq!(
        capability::load(crate_dir.join(DEFAULT_RECORD), &on).map(drop),
        Err(RecordRefused::Unevidenced(Capability::ToolCalling)),
        "the record of the default adapter evidences no tool calling"
    );
}
