//! The usage of a subgraph's child run rolls up into the parent run's total
//! (`story:usage-subgraph-rollup`).
//!
//! The child is the A-02 text pipeline: the mock model between the ingress `user text` and the
//! egress `reply`, its `usage` port exposed as the egress `model usage`. The parent is a subgraph
//! node over that child, its ports `user text` and `reply` exposed under their names, and its one
//! `usage` port, which forwards every usage egress of the child, exposed as `usage`. A session over
//! the parent answering one turn delivers the model's usage record once, naming the child run and
//! not the session's; the session holds one run, the parent, whose total and the session's total
//! each count that record once, and keeps no total of the child run.

#![allow(dead_code)]

mod text;

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use inference_core::id::{GenerationId, RunId};
use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_core::usage::{KindTotal, QuantityKind, UsageRecorded, UsageTotal};
use inference_facade::events::SessionEvent;
use inference_facade::live::LiveSessions;
use inference_llm::llm_core::Usage;
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortId, PortTraits, USAGE_PORT,
    modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{SubgraphHandle, SubgraphNode};
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{MockGenerator, ModelNode};

use text::{Application, EGRESS, INGRESS, block_on, generator, reply};

/// The name the child exposes its model's `usage` port under, which no port of the subgraph
/// node carries.
const MODEL_USAGE: &str = "model usage";

struct Fixture {
    profile: DeploymentProfile,
    adapter: Adapter,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        (self.adapter.adapter_id() == adapter_id).then_some(&self.adapter)
    }
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

const RECORDS: PortTraits = PortTraits {
    incremental: false,
    ..TEXT
};

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    }
}

fn capacity() -> NonZeroUsize {
    NonZeroUsize::new(16).expect("16")
}

/// The declaration holding the planned child and parent, and the child model's `usage` port.
struct Planned {
    declaration: Declaration,
    child: PlannedPipeline,
    parent: PlannedPipeline,
    usage: PortId,
}

fn planned(generator: &MockGenerator) -> Planned {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let adapter_id = generator.adapter().adapter_id();
    let catalog = Fixture {
        profile,
        adapter: generator.adapter().clone(),
    };
    let mut declaration = Declaration::new();
    let (child_id, usage) = declaration
        .build(&catalog, profile_id, spec("text session"), |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
            let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
            let usage = pipeline.output::<modality::Structured>(&model, USAGE_PORT, RECORDS);
            pipeline.expose(&prompt, INGRESS, capacity(), OverloadPolicy::Pause);
            pipeline.expose(&answer, EGRESS, capacity(), OverloadPolicy::Pause);
            pipeline.expose(&usage, MODEL_USAGE, capacity(), OverloadPolicy::Pause);
            pipeline
                .bind(&model, adapter_id)
                .expect("the catalog holds the mock LLM's adapter");
            usage.id()
        })
        .expect("the profile is declared");
    let validated = declaration.validate(child_id).expect("valid");
    let child = declaration.plan(validated, &catalog).expect("planned");
    let (parent_id, ()) = declaration
        .build(&catalog, profile_id, spec("subgraph session"), |pipeline| {
            let subgraph = pipeline
                .subgraph("frontend", NodeRole::Transform, child_id)
                .expect("the child is Planned");
            let into = pipeline.input::<modality::Text>(&subgraph, INGRESS, TEXT);
            let out = pipeline.output::<modality::Text>(&subgraph, EGRESS, TEXT);
            let usage = pipeline.output::<modality::Structured>(&subgraph, USAGE_PORT, RECORDS);
            pipeline.expose(&into, INGRESS, capacity(), OverloadPolicy::Pause);
            pipeline.expose(&out, EGRESS, capacity(), OverloadPolicy::Pause);
            pipeline.expose(&usage, USAGE_PORT, capacity(), OverloadPolicy::Pause);
        })
        .expect("the profile is declared");
    let validated = declaration.validate(parent_id).expect("valid");
    let parent = declaration
        .plan(validated, &catalog)
        .expect("the subgraph node's `usage` port forwards the child's `model usage`");
    Planned {
        declaration,
        child,
        parent,
        usage,
    }
}

/// The total of `records`, added one by one.
fn total_of<'a>(records: impl IntoIterator<Item = &'a UsageRecorded>) -> UsageTotal {
    let mut total = UsageTotal::new();
    for recorded in records {
        total.add(&recorded.quantities, recorded.r#final);
    }
    total
}

#[test]
fn an_llm_inside_a_subgraph_node_is_counted_once_in_the_parent_run_and_session_totals() {
    let generator = generator(Vec::<String>::new()).reporting([Some(Usage {
        input_tokens: Some(7),
        output_tokens: Some(2),
        ..Usage::default()
    })]);
    let planned = planned(&generator);
    let handle: Arc<Mutex<Option<SubgraphHandle>>> = Arc::default();
    let implement = {
        let declaration = planned.declaration.clone();
        let child = planned.child.clone();
        let usage = planned.usage;
        let handle = Arc::clone(&handle);
        move |run_id: RunId, _: &Node| {
            let generator = generator.clone();
            let node = SubgraphNode::new(
                run_id,
                &declaration,
                &child,
                HashMap::<GenerationId, i64>::new(),
                Box::new(move |child_run: RunId, _: &Node| {
                    Box::new(
                        ModelNode::new(child_run, generator.clone(), reply, RunLog::new())
                            .with_usage(usage),
                    ) as Box<dyn NodeImplementation<PipelinePayload>>
                }),
            );
            *handle.lock().unwrap_or_else(PoisonError::into_inner) = Some(node.handle());
            Box::new(node) as Box<dyn NodeImplementation<PipelinePayload>>
        }
    };
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &planned.declaration,
        &planned.parent,
        InteractionMode::TurnBased,
        implement,
    ))
    .expect("a session opens over the parent");
    let parent_run = session.run_id();
    let child_run = handle
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(SubgraphHandle::child_run_id)
        .expect("the subgraph node prepared its child run");
    assert_ne!(child_run, parent_run);
    let drive = thread::spawn(move || block_on(driver.drive()));

    let mut application = Application::new(parent_run);
    block_on(session.send(application.text("hello"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    block_on(session.close()).expect("a Ready session drains");
    let mut received = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        received.push(event);
    }
    let ended = drive
        .join()
        .expect("no panic")
        .expect("the parent run ends");
    assert_eq!(ended.state(), RunState::Completed);
    assert!(
        received
            .iter()
            .any(|event| matches!(event, SessionEvent::TextSegmentCommitted(_))),
        "the child's reply crossed: {received:#?}"
    );

    let records: Vec<&UsageRecorded> = received
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded),
            _ => None,
        })
        .collect();
    let [record] = records.as_slice() else {
        panic!("the LLM's one call delivers one record, once: {received:#?}")
    };
    assert_eq!(
        record.run_id,
        Some(child_run),
        "the record keeps the child run it was made in"
    );

    let usage = session.usage();
    let total = total_of(records.iter().copied());
    assert_eq!(
        usage.runs,
        [(parent_run, total.clone())],
        "the session holds the parent run alone, and its total includes the child's record"
    );
    assert_eq!(usage.run(child_run), None, "no total of the child run");
    assert_eq!(usage.session, total, "the session counts the record once");
    assert_eq!(usage.session.records(), 1);
    assert_eq!(
        usage.session.kind(QuantityKind::InputTextTokens),
        Some(KindTotal {
            known: 7,
            unknown: 0,
            records: 1,
            partial: 0,
        })
    );
}
