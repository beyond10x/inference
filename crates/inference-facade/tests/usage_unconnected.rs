//! Usage that reaches no edge is still counted (`story:usage-unconnected-port-counted`).
//!
//! A model node whose `usage` port no edge or exposure leaves, and a subgraph node whose `usage`
//! port no parent edge leaves, still have their usage records counted in the total of the run the
//! session holds and in the session's total; no egress carries those records, so none is
//! delivered. A record whose port has an edge is counted once, as before, beside them.
//!
//! | case | acceptance |
//! |---|---|
//! | [`a_model_node_with_no_usage_edge_is_counted_in_the_run_and_session_totals`] | the model's record counts once in both totals and is not delivered |
//! | [`a_subgraph_node_with_no_usage_edge_is_counted_in_the_parent_run_and_session_totals`] | the child's record counts once under the parent run |
//! | [`a_connected_usage_port_beside_an_unconnected_one_is_counted_once`] | two models, one exposed and one not: two records counted, the exposed one delivered once |
//! | [`a_child_model_with_no_usage_edge_rolls_up_through_a_connected_usage_port_once`] | a child model with no edge in its child reaches the parent through the subgraph node's `usage` port, counted once |
//! | [`a_subgraph_node_with_no_usage_port_over_an_unconnected_child_marks_the_totals_partial`] | the one place no port can carry the usage: the run's and the session's totals name the subgraph node and are not complete |

#![allow(dead_code)]

mod text;

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::thread;

use inference_core::id::{GenerationId, NodeId, RunId};
use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_core::usage::{KindTotal, QuantityKind, UsageRecorded};
use inference_facade::events::SessionEvent;
use inference_facade::live::LiveSessions;
use inference_facade::usage::SessionUsage;
use inference_llm::llm_core::Usage;
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineBuilder, PipelineId, PipelineSpec, PlannedPipeline, PortId,
    PortTraits, USAGE_PORT, modality,
};
use inference_runtime::SubgraphNode;
use inference_runtime::run::RunState;
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{MockGenerator, ModelNode};

use text::{Application, EGRESS, INGRESS, block_on, generator, reply};

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

const OPTIONAL_RECORDS: PortTraits = PortTraits {
    essentiality: Essentiality::Optional,
    ..RECORDS
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

fn catalog(generator: &MockGenerator) -> (Fixture, ProfileId) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    (
        Fixture {
            profile,
            adapter: generator.adapter().clone(),
        },
        profile_id,
    )
}

/// A mock model reporting one call of `input` input tokens and 2 output tokens.
fn reporting(input: u64) -> MockGenerator {
    generator(Vec::<String>::new()).reporting([Some(Usage {
        input_tokens: Some(input),
        output_tokens: Some(2),
        ..Usage::default()
    })])
}

/// Declares a model node `name` bound to `adapter`, its `prompt` exposed as `ingress` and its
/// `answer` as `egress`; its `usage` port exposed as `usage` when given, otherwise optional and
/// left with no edge. Answers the node and its `usage` port.
fn model(
    pipeline: &mut PipelineBuilder<'_, '_>,
    adapter: AdapterId,
    name: &str,
    (ingress, egress): (&str, &str),
    usage: Option<&str>,
) -> (NodeId, PortId) {
    let node = pipeline.node(name, NodeRole::Model, vec![Capability::OutputStreaming]);
    let prompt = pipeline.input::<modality::Text>(&node, "prompt", TEXT);
    let answer = pipeline.output::<modality::Text>(&node, "answer", TEXT);
    pipeline.expose(&prompt, ingress, capacity(), OverloadPolicy::Pause);
    pipeline.expose(&answer, egress, capacity(), OverloadPolicy::Pause);
    let used = match usage {
        Some(exposed) => {
            let used = pipeline.output::<modality::Structured>(&node, USAGE_PORT, RECORDS);
            pipeline.expose(&used, exposed, capacity(), OverloadPolicy::Pause);
            used
        }
        None => pipeline.output::<modality::Structured>(&node, USAGE_PORT, OPTIONAL_RECORDS),
    };
    pipeline
        .bind(&node, adapter)
        .expect("the catalog holds the mock LLM's adapter");
    (node.id(), used.id())
}

/// The mock model on `generator`, given its `usage` port.
fn model_node(
    run_id: RunId,
    generator: &MockGenerator,
    usage: PortId,
) -> Box<dyn NodeImplementation<PipelinePayload>> {
    Box::new(ModelNode::new(run_id, generator.clone(), reply, RunLog::new()).with_usage(usage))
}

/// What one turn of a session left: the events it delivered, its usage view and its run.
struct Turned {
    run_id: RunId,
    received: Vec<SessionEvent>,
    usage: SessionUsage,
}

impl Turned {
    fn delivered(&self) -> Vec<&UsageRecorded> {
        self.received
            .iter()
            .filter_map(|event| match event {
                SessionEvent::UsageRecorded(recorded) => Some(recorded),
                _ => None,
            })
            .collect()
    }
}

/// Opens a session over `planned`, sends `hello` on every ingress of `ingresses`, commits,
/// closes, and receives until the session ends.
fn one_turn(
    declaration: &Declaration,
    planned: &PlannedPipeline,
    ingresses: &[&str],
    implement: impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<PipelinePayload>>,
) -> Turned {
    let sessions = LiveSessions::new();
    let (mut session, driver) =
        block_on(sessions.open(declaration, planned, InteractionMode::TurnBased, implement))
            .expect("a session opens");
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    for ingress in ingresses {
        let mut envelope = Application::new(run_id).text("hello");
        envelope.port = (*ingress).to_owned();
        block_on(session.send(envelope)).expect("sent");
    }
    block_on(session.commit_input()).expect("committed");
    block_on(session.close()).expect("a Ready session drains");
    let mut received = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        received.push(event);
    }
    let ended = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(ended.state(), RunState::Completed, "{received:#?}");
    Turned {
        run_id,
        received,
        usage: session.usage(),
    }
}

/// The input token total of `records` records summing `known`.
const fn input_tokens(known: i128, records: u64) -> Option<KindTotal> {
    Some(KindTotal {
        known,
        unknown: 0,
        records,
        partial: 0,
    })
}

/// Asserts the run's total and the session's each count `records` records of `known` input
/// tokens, the session holding that run alone.
fn assert_counted(turned: &Turned, known: i128, records: u64) {
    let usage = &turned.usage;
    let [(held, run)] = usage.runs.as_slice() else {
        panic!("the session holds one run: {usage:#?}")
    };
    assert_eq!(*held, turned.run_id);
    assert_eq!(run.records(), records, "the run's total: {usage:#?}");
    assert_eq!(
        run.kind(QuantityKind::InputTextTokens),
        input_tokens(known, records)
    );
    assert_eq!(usage.session, *run, "the session's total is the run's");
}

#[test]
fn a_model_node_with_no_usage_edge_is_counted_in_the_run_and_session_totals() {
    let generator = reporting(7);
    let (catalog, profile_id) = catalog(&generator);
    let adapter = generator.adapter().adapter_id();
    let mut declaration = Declaration::new();
    let (pipeline, (_, usage)) = declaration
        .build(
            &catalog,
            profile_id,
            spec("unconnected model"),
            |pipeline| model(pipeline, adapter, "model", (INGRESS, EGRESS), None),
        )
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");

    let turned = one_turn(&declaration, &planned, &[INGRESS], move |run_id, _| {
        model_node(run_id, &generator, usage)
    });

    assert!(
        turned.delivered().is_empty(),
        "no exposure carries the record, so none is delivered: {:#?}",
        turned.received
    );
    assert_counted(&turned, 7, 1);
}

/// The child of the subgraph cases: one model between `user text` and `reply`, its `usage` port
/// exposed as `model usage` when `exposed`, otherwise with no edge; and, given `unconnected`, a
/// second node, a model whose `usage` port has no edge in the child, between `other text` and
/// `other reply`. Answers the child and the `usage` port of each model.
fn child(
    declaration: &mut Declaration,
    catalog: &Fixture,
    profile_id: ProfileId,
    adapter: AdapterId,
    (exposed, unconnected): (bool, bool),
) -> (PlannedPipeline, PipelineId, Vec<(NodeId, PortId)>) {
    let (child_id, models) = declaration
        .build(catalog, profile_id, spec("child"), |pipeline| {
            let mut models = vec![model(
                pipeline,
                adapter,
                "model",
                (INGRESS, EGRESS),
                exposed.then_some("model usage"),
            )];
            if unconnected {
                models.push(model(
                    pipeline,
                    adapter,
                    "other",
                    ("other text", "other reply"),
                    None,
                ));
            }
            models
        })
        .expect("the profile is declared");
    let validated = declaration.validate(child_id).expect("valid");
    let planned = declaration.plan(validated, catalog).expect("planned");
    (planned, child_id, models)
}

/// A parent of one subgraph node over `child`, its ports named as the child's exposures
/// `ingresses` and `egresses` and exposed under those names, and its `usage` port exposed as
/// `usage` when `usage` is `Some(true)`, optional with no edge when `Some(false)`; given `None`,
/// the node has no `usage` port.
fn parent(
    declaration: &mut Declaration,
    catalog: &Fixture,
    profile_id: ProfileId,
    child: PipelineId,
    (ingresses, egresses): (&[&str], &[&str]),
    usage: Option<bool>,
) -> (PlannedPipeline, NodeId) {
    let (parent_id, node) = declaration
        .build(catalog, profile_id, spec("parent"), |pipeline| {
            let subgraph = pipeline
                .subgraph("frontend", NodeRole::Transform, child)
                .expect("the child is Planned");
            for name in ingresses {
                let into = pipeline.input::<modality::Text>(&subgraph, *name, TEXT);
                pipeline.expose(&into, *name, capacity(), OverloadPolicy::Pause);
            }
            for name in egresses {
                let out = pipeline.output::<modality::Text>(&subgraph, *name, TEXT);
                pipeline.expose(&out, *name, capacity(), OverloadPolicy::Pause);
            }
            match usage {
                Some(true) => {
                    let used =
                        pipeline.output::<modality::Structured>(&subgraph, USAGE_PORT, RECORDS);
                    pipeline.expose(&used, USAGE_PORT, capacity(), OverloadPolicy::Pause);
                }
                Some(false) => {
                    let _unconnected = pipeline.output::<modality::Structured>(
                        &subgraph,
                        USAGE_PORT,
                        OPTIONAL_RECORDS,
                    );
                }
                None => {}
            }
            subgraph.id()
        })
        .expect("the profile is declared");
    let validated = declaration.validate(parent_id).expect("valid");
    let planned = declaration
        .plan(validated, catalog)
        .expect("the subgraph node's ports match the child's exposures");
    (planned, node)
}

/// The subgraph node over `child`, each model of whose made by `models` on its generator.
fn subgraph_node(
    run_id: RunId,
    declaration: &Declaration,
    child: &PlannedPipeline,
    models: Vec<(NodeId, PortId, MockGenerator)>,
) -> Box<dyn NodeImplementation<PipelinePayload>> {
    Box::new(SubgraphNode::new(
        run_id,
        declaration,
        child,
        HashMap::<GenerationId, i64>::new(),
        Box::new(move |child_run: RunId, node: &Node| {
            let (_, usage, generator) = models
                .iter()
                .find(|(model, _, _)| *model == node.node_id())
                .expect("every node of the child is a model");
            model_node(child_run, generator, *usage)
        }),
    ))
}

#[test]
fn a_subgraph_node_with_no_usage_edge_is_counted_in_the_parent_run_and_session_totals() {
    let generator = reporting(7);
    let (catalog, profile_id) = catalog(&generator);
    let adapter = generator.adapter().adapter_id();
    let mut declaration = Declaration::new();
    let (child, child_id, models) = child(
        &mut declaration,
        &catalog,
        profile_id,
        adapter,
        (true, false),
    );
    let (planned, _) = parent(
        &mut declaration,
        &catalog,
        profile_id,
        child_id,
        (&[INGRESS], &[EGRESS]),
        Some(false),
    );
    let models: Vec<(NodeId, PortId, MockGenerator)> = models
        .into_iter()
        .map(|(node, usage)| (node, usage, generator.clone()))
        .collect();

    let turned = {
        let declaration_ = declaration.clone();
        one_turn(&declaration, &planned, &[INGRESS], move |run_id, _| {
            subgraph_node(run_id, &declaration_, &child, models.clone())
        })
    };

    assert!(
        turned.delivered().is_empty(),
        "no parent edge carries the record, so none is delivered: {:#?}",
        turned.received
    );
    assert_counted(&turned, 7, 1);
}

#[test]
fn a_connected_usage_port_beside_an_unconnected_one_is_counted_once() {
    let connected = reporting(7);
    let unconnected = reporting(5);
    let (catalog, profile_id) = catalog(&connected);
    let adapter = connected.adapter().adapter_id();
    let mut declaration = Declaration::new();
    let (pipeline, models) = declaration
        .build(&catalog, profile_id, spec("two models"), |pipeline| {
            [
                model(pipeline, adapter, "a", (INGRESS, EGRESS), Some("usage")),
                model(pipeline, adapter, "b", ("b text", "b reply"), None),
            ]
        })
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");

    let turned = one_turn(
        &declaration,
        &planned,
        &[INGRESS, "b text"],
        move |run_id, node| {
            let [(a, a_usage), (_, b_usage)] = models;
            if node.node_id() == a {
                model_node(run_id, &connected, a_usage)
            } else {
                model_node(run_id, &unconnected, b_usage)
            }
        },
    );

    let delivered = turned.delivered();
    let [record] = delivered.as_slice() else {
        panic!(
            "the exposed port's record is delivered once: {:#?}",
            turned.received
        )
    };
    assert_eq!(
        record
            .quantities
            .iter()
            .find(|quantity| quantity.kind() == QuantityKind::InputTextTokens)
            .and_then(|quantity| quantity.value()),
        Some(7)
    );
    assert_counted(&turned, 12, 2);
}

#[test]
fn a_child_model_with_no_usage_edge_rolls_up_through_a_connected_usage_port_once() {
    let connected = reporting(7);
    let unconnected = reporting(5);
    let (catalog, profile_id) = catalog(&connected);
    let adapter = connected.adapter().adapter_id();
    let mut declaration = Declaration::new();
    let (child, child_id, models) = child(
        &mut declaration,
        &catalog,
        profile_id,
        adapter,
        (true, true),
    );
    let (planned, _) = parent(
        &mut declaration,
        &catalog,
        profile_id,
        child_id,
        (&[INGRESS, "other text"], &[EGRESS, "other reply"]),
        Some(true),
    );
    let models: Vec<(NodeId, PortId, MockGenerator)> = models
        .into_iter()
        .zip([connected, unconnected])
        .map(|((node, usage), generator)| (node, usage, generator))
        .collect();

    let turned = {
        let declaration_ = declaration.clone();
        one_turn(
            &declaration,
            &planned,
            &[INGRESS, "other text"],
            move |run_id, _| subgraph_node(run_id, &declaration_, &child, models.clone()),
        )
    };

    let delivered = turned.delivered();
    assert_eq!(
        delivered.len(),
        2,
        "both child records cross the connected `usage` port, each once: {:#?}",
        turned.received
    );
    assert_counted(&turned, 12, 2);
}

#[test]
fn a_subgraph_node_with_no_usage_port_over_an_unconnected_child_marks_the_totals_partial() {
    let generator = reporting(7);
    let (catalog, profile_id) = catalog(&generator);
    let adapter = generator.adapter().adapter_id();
    let mut declaration = Declaration::new();
    let (child, child_id, models) = child(
        &mut declaration,
        &catalog,
        profile_id,
        adapter,
        (false, false),
    );
    let (planned, subgraph) = parent(
        &mut declaration,
        &catalog,
        profile_id,
        child_id,
        (&[INGRESS], &[EGRESS]),
        None,
    );
    let models: Vec<(NodeId, PortId, MockGenerator)> = models
        .into_iter()
        .map(|(node, usage)| (node, usage, generator.clone()))
        .collect();

    let turned = {
        let declaration_ = declaration.clone();
        one_turn(&declaration, &planned, &[INGRESS], move |run_id, _| {
            subgraph_node(run_id, &declaration_, &child, models.clone())
        })
    };

    let usage = &turned.usage;
    let [(held, run)] = usage.runs.as_slice() else {
        panic!("the session holds one run: {usage:#?}")
    };
    assert_eq!(*held, turned.run_id);
    assert_eq!(
        run.records(),
        0,
        "the subgraph node has no port to pass the usage on"
    );
    assert_eq!(
        run.uncounted().collect::<Vec<_>>(),
        [subgraph],
        "the run's total names the node whose usage it misses"
    );
    assert!(!run.complete(), "a total that misses usage is partial");
    assert_eq!(usage.session, *run, "the session's total is marked too");
}
