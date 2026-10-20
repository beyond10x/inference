//! The adapter against the loopback fixture of `common`, which replays a stream recorded from a
//! running `llama-server`. No server is started and no weights are read.
//!
//! The capability record committed in `capability/` is checked here against the declaration of
//! the adapter with its `tools` option on; its `output_streaming` evidence names the tests of this
//! file that observe it, its `tool_calling` evidence those of `tool_proposals`.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::{DELTAS, Fixture, SERVED_MODEL};
use inference_core::id::{NodeId, RunId};
use inference_core::model::{Adapter, Capability, ExecutionForm, Locality, Model};
use inference_llama_server::{
    BACKEND, CHECKPOINT, CHECKSUM, CODE_LICENSE, Config, LlamaServer, SOURCE, SOURCE_REVISION,
    WEIGHT_LICENSE, binding, declare_adapter, declare_model,
};
use inference_llm::llm_core::{Item, Model as _, TurnRequest};
use inference_llm::{
    CapabilityNotClaimed, GenerationEvent, GenerationHandle, GenerationRequest, Generator,
    InputRefused,
};
use inference_testkit::model::capability;

const PROMPT: &str = "Say hello in three words.";

/// The record committed for the reference machine, CPU only.
const RECORD: &str = "capability/reference-machine-cpu.json";

/// The model and adapter declared for a server configured by `config`.
fn declared(config: &Config) -> (Model, Adapter) {
    let (model, _) = Model::declare(declare_model());
    let (adapter, _) =
        Adapter::declare(declare_adapter(model.model_id(), config)).expect("one slot is positive");
    (model, adapter)
}

fn generator(fixture: &Fixture) -> Generator {
    let config = Config::new(fixture.base_url(), SERVED_MODEL);
    let (_, adapter) = declared(&config);
    LlamaServer::new(&config)
        .expect("the fixture's base URL makes a binding")
        .generator(adapter)
        .expect("the binding declares what the adapter claims")
}

fn start(generator: &mut Generator) -> GenerationHandle {
    generator
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn: TurnRequest::new(SERVED_MODEL, vec![Item::user(PROMPT)]),
        })
        .expect("no generation runs yet")
}

/// Takes events up to and including the first text delta.
fn until_first_delta(handle: &mut GenerationHandle) -> Vec<GenerationEvent> {
    let mut events = Vec::new();
    for event in handle.by_ref() {
        let delta = matches!(event, GenerationEvent::TextDelta(_));
        events.push(event);
        if delta {
            return events;
        }
    }
    panic!("the generation ended before a delta: {events:?}")
}

fn names(events: &[GenerationEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            GenerationEvent::Started(_) => "StartGeneration",
            GenerationEvent::TextDelta(_) => "EmitTextDelta",
            GenerationEvent::SegmentCommitted(_) => "CommitSegment",
            GenerationEvent::Completed(_) => "CompleteGeneration",
            GenerationEvent::Cancelled(..) => "CancelGeneration",
        })
        .collect()
}

#[test]
fn a_delta_reaches_the_caller_while_the_server_still_holds_the_rest_of_the_stream() {
    let mut fixture = Fixture::holding();
    let mut generator = generator(&fixture);
    let mut handle = start(&mut generator);

    let first = until_first_delta(&mut handle);
    assert_eq!(names(&first), ["StartGeneration", "EmitTextDelta"]);
    let GenerationEvent::TextDelta(delta) = &first[1] else {
        unreachable!("the last event taken is a delta")
    };
    assert_eq!(delta.text, DELTAS[0]);
    assert_eq!(
        handle.output().generation().state(),
        inference_core::generation::GenerationState::Active,
        "the delta is delivered while the generation is Active and the turn is still running"
    );

    fixture.release();
    let rest: Vec<GenerationEvent> = handle.by_ref().collect();
    let mut expected = vec!["CommitSegment"];
    for _ in &DELTAS[1..] {
        expected.extend(["EmitTextDelta", "CommitSegment"]);
    }
    expected.push("CompleteGeneration");
    assert_eq!(names(&rest), expected, "{rest:?}");
    let texts: Vec<&str> = first
        .iter()
        .chain(&rest)
        .filter_map(|event| match event {
            GenerationEvent::TextDelta(delta) => Some(delta.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, DELTAS, "each delta of the server is one segment");
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
}

#[test]
fn input_offered_while_the_generation_is_active_is_refused_and_sends_nothing() {
    let mut fixture = Fixture::holding();
    let mut generator = generator(&fixture);
    let mut handle = start(&mut generator);
    until_first_delta(&mut handle);

    assert_eq!(
        handle.offer_input("and once more".to_owned()),
        Err(InputRefused::CapabilityNotClaimed(CapabilityNotClaimed {
            capability: Capability::ConcurrentInputOutput,
        }))
    );

    fixture.release();
    let rest: Vec<GenerationEvent> = handle.by_ref().collect();
    assert!(
        matches!(rest.last(), Some(GenerationEvent::Completed(_))),
        "{rest:?}"
    );
    assert_eq!(
        fixture.requests().len(),
        1,
        "the refused input reached no second request"
    );
}

#[test]
fn the_request_is_an_anonymous_streamed_chat_completion_of_the_served_model() {
    let fixture = Fixture::replaying();
    let mut generator = generator(&fixture);
    let events: Vec<GenerationEvent> = start(&mut generator).collect();
    assert!(
        matches!(events.last(), Some(GenerationEvent::Completed(_))),
        "{events:?}"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert!(
        request.head.starts_with("POST /v1/chat/completions "),
        "{}",
        request.head
    );
    assert!(
        !request.head.to_ascii_lowercase().contains("authorization"),
        "no authorization header: {}",
        request.head
    );
    let body = request.json();
    assert_eq!(body["stream"], serde_json::json!(true));
    assert_eq!(body["model"], serde_json::json!(SERVED_MODEL));
    assert_eq!(
        body["messages"],
        serde_json::json!([{"role": "user", "content": PROMPT}])
    );
}

#[test]
fn the_adapter_claims_output_streaming_only_and_the_binding_declares_it() {
    let config = Config::new("http://127.0.0.1:8080/v1", SERVED_MODEL);
    let (model, adapter) = declared(&config);
    assert_eq!(adapter.capabilities(), [Capability::OutputStreaming]);
    assert_eq!(adapter.backend(), BACKEND);
    assert_eq!(adapter.execution_form(), ExecutionForm::LocalService);
    assert_eq!(adapter.locality(), Locality::LocalWorker);
    assert_eq!(adapter.max_concurrency(), 1);
    assert_eq!(adapter.model_id(), model.model_id());

    let bound = binding(&config).expect("a loopback base URL makes a binding");
    let declared = bound.capabilities();
    assert!(declared.output_streaming);
    assert!(!declared.input_during_output);
    assert!(!declared.tools);

    let server = LlamaServer::new(&config).expect("a binding");
    assert_eq!(server.capabilities(), declared);
    assert_eq!(server.provenance().model.as_str(), SERVED_MODEL);
    let generator = Generator::new(adapter, Arc::new(server))
        .expect("the binding declares output streaming, the only claim");
    assert_eq!(
        generator.adapter().capabilities(),
        [Capability::OutputStreaming]
    );
}

#[test]
fn the_model_declaration_names_the_pinned_checkpoint_its_licences_and_checksum() {
    let declaration = declare_model();
    assert_eq!(declaration.checkpoint, CHECKPOINT);
    assert_eq!(declaration.code_license, CODE_LICENSE);
    assert_eq!(declaration.weight_license, WEIGHT_LICENSE);
    assert_eq!(declaration.checksum, CHECKSUM);
    assert_eq!(declaration.provenance, SOURCE);
    assert_eq!(declaration.source_revision, SOURCE_REVISION);
    assert_eq!(CODE_LICENSE, "MIT");
    assert_eq!(WEIGHT_LICENSE, "Apache-2.0");
    assert_eq!(
        CHECKSUM,
        "sha256:6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e"
    );
}

#[test]
fn the_committed_capability_record_checks_against_the_adapter_declaration() {
    let mut config = Config::new("http://127.0.0.1:8080/v1", SERVED_MODEL);
    config.tools = true;
    let (_, adapter) = declared(&config);
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let record = capability::load(crate_dir.join(RECORD), &adapter)
        .unwrap_or_else(|refused| panic!("{refused}"));
    assert_eq!(record.adapter_id, adapter.adapter_id());
    assert_eq!(record.resource_profile.gpu_memory_mib, None, "CPU only");
    assert!(record.audio_formats.is_empty(), "a text model");

    for evidence in &record.evidence {
        for test in &evidence.tests {
            let (file, name) = test
                .split_once("::")
                .unwrap_or_else(|| panic!("`{test}` names no test file"));
            let source =
                std::fs::read_to_string(crate_dir.join("tests").join(format!("{file}.rs")))
                    .unwrap_or_else(|error| panic!("`{test}`: {error}"));
            assert!(
                source.contains(&format!("fn {name}()")),
                "the record names `{test}`, which is not a test of this crate"
            );
        }
    }
}
