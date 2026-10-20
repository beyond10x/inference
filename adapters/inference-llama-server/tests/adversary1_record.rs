//! Adversary cases of the adapter's declarations: its committed capability record against the
//! adapter `story:llama-server-adapter` declares, and the output limit of its configuration on
//! the wire.

mod common;

use std::fs;
use std::path::Path;

use common::{Fixture, SERVED_MODEL};
use inference_core::id::{NodeId, RunId};
use inference_core::model::{Adapter, Model};
use inference_llama_server::{Config, LlamaServer, declare_adapter, declare_model};
use inference_llm::llm_core::{Item, TurnRequest};
use inference_llm::{GenerationEvent, GenerationRequest};
use inference_testkit::model::capability;

fn adapter(config: &Config) -> Adapter {
    let (model, _) = Model::declare(declare_model());
    Adapter::declare(declare_adapter(model.model_id(), config))
        .expect("one slot")
        .0
}

/// `story:llama-server-adapter` declares an adapter claiming exactly `output_streaming` (the
/// default `Config`), and its acceptance checks the committed record against that declaration.
/// The design keeps one record per adapter and hardware target, so the default adapter needs a
/// record in `capability/` that its check accepts.
#[test]
fn a_committed_record_checks_against_the_default_adapter_declaration() {
    let default = adapter(&Config::new("http://127.0.0.1:8080/v1", SERVED_MODEL));
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("capability");
    let mut refusals = Vec::new();
    for entry in fs::read_dir(&dir).expect("the record directory") {
        let path = entry.expect("an entry").path();
        match capability::load(&path, &default) {
            Ok(_) => return,
            Err(refused) => refusals.push(format!("{}: {refused}", path.display())),
        }
    }
    panic!("no committed record checks against the default adapter: {refusals:?}");
}

/// `Config::max_output_tokens` is documented as "the most tokens one turn may produce", and
/// `Config::new` as "at most 1024 tokens per turn". A turn built the way every caller of this
/// crate builds it, `TurnRequest::new`, must carry that bound to the server, which otherwise
/// generates until its context is full.
#[test]
fn a_turn_of_the_default_config_asks_the_server_for_at_most_max_output_tokens() {
    let fixture = Fixture::replaying();
    let config = Config::new(fixture.base_url(), SERVED_MODEL);
    let mut generator = LlamaServer::new(&config)
        .expect("a binding")
        .generator(adapter(&config))
        .expect("the binding declares the claim");
    let events: Vec<GenerationEvent> = generator
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn: TurnRequest::new(SERVED_MODEL, vec![Item::user("Say hello in three words.")]),
        })
        .expect("a slot")
        .collect();
    assert!(
        matches!(events.last(), Some(GenerationEvent::Completed(_))),
        "{events:?}"
    );
    let body = fixture.requests()[0].json();
    assert_eq!(
        body["max_completion_tokens"],
        serde_json::json!(config.max_output_tokens),
        "the request bounds the turn's output: {body}"
    );
}
