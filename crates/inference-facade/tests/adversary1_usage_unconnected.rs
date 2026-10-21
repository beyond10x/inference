//! Adversary pass 1 on `story:usage-unconnected-port-counted`: two subgraph nodes with no `usage`
//! port over one child pipeline that keeps usage in a sink, and a record rolling up two levels
//! through a `usage` port with no edge at each level.
//!
//! | case | what it asserts |
//! |---|---|
//! | [`two_subgraph_nodes_with_no_usage_port_over_one_child_are_both_named`] | `uncounted_usage` and the run's total name every node whose usage is dropped, not the first over a child pipeline only |
//! | [`a_record_rolls_up_two_levels_through_unconnected_usage_ports_once`] | a grandchild's record crosses an inner subgraph node's `usage` port with no edge and a middle one's, and counts once beside the middle child's own |

#![allow(dead_code)]

mod text;

use std::collections::{BTreeSet, HashMap};
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
use inference_runtime::run::RunState;
use inference_runtime::{SubgraphNode, uncounted_usage};
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

fn reporting(input: u64) -> MockGenerator {
    generator(Vec::<String>::new()).reporting([Some(Usage {
        input_tokens: Some(input),
        output_tokens: Some(2),
        ..Usage::default()
    })])
}

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

fn model_node(
    run_id: RunId,
    generator: &MockGenerator,
    usage: PortId,
) -> Box<dyn NodeImplementation<PipelinePayload>> {
    Box::new(ModelNode::new(run_id, generator.clone(), reply, RunLog::new()).with_usage(usage))
}

/// A child of one model between `user text` and `reply`, its `usage` port exposed as
/// `model usage` when `exposed`, otherwise with no edge.
fn child(
    declaration: &mut Declaration,
    catalog: &Fixture,
    profile_id: ProfileId,
    adapter: AdapterId,
    exposed: bool,
) -> (PlannedPipeline, PipelineId, (NodeId, PortId)) {
    let (child_id, model) = declaration
        .build(catalog, profile_id, spec("child"), |pipeline| {
            model(
                pipeline,
                adapter,
                "model",
                (INGRESS, EGRESS),
                exposed.then_some("model usage"),
            )
        })
        .expect("the profile is declared");
    let validated = declaration.validate(child_id).expect("valid");
    let planned = declaration.plan(validated, catalog).expect("planned");
    (planned, child_id, model)
}

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

#[test]
fn two_subgraph_nodes_with_no_usage_port_over_one_child_are_both_named() {
    let generator = reporting(7);
    let (catalog, profile_id) = catalog(&generator);
    let adapter = generator.adapter().adapter_id();
    let mut declaration = Declaration::new();
    let (child, child_id, (model_id, usage)) =
        child(&mut declaration, &catalog, profile_id, adapter, false);
    let (parent_id, nodes) = declaration
        .build(&catalog, profile_id, spec("parent"), |pipeline| {
            let mut nodes = Vec::new();
            for (name, ingress, egress) in [
                ("frontend", INGRESS, EGRESS),
                ("backend", "back text", "back reply"),
            ] {
                let subgraph = pipeline
                    .subgraph(name, NodeRole::Transform, child_id)
                    .expect("the child is Planned");
                let into = pipeline.input::<modality::Text>(&subgraph, INGRESS, TEXT);
                pipeline.expose(&into, ingress, capacity(), OverloadPolicy::Pause);
                let out = pipeline.output::<modality::Text>(&subgraph, EGRESS, TEXT);
                pipeline.expose(&out, egress, capacity(), OverloadPolicy::Pause);
                nodes.push(subgraph.id());
            }
            nodes
        })
        .expect("the profile is declared");
    let validated = declaration.validate(parent_id).expect("valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("each subgraph node's ports match the child's exposures");
    let expected: BTreeSet<NodeId> = nodes.iter().copied().collect();

    let named: BTreeSet<NodeId> = uncounted_usage(&declaration, planned.pipeline_id())
        .into_iter()
        .collect();
    assert_eq!(
        named, expected,
        "both subgraph nodes drop the usage their child keeps in its sink, so both are named"
    );

    let turned = {
        let declaration_ = declaration.clone();
        one_turn(
            &declaration,
            &planned,
            &[INGRESS, "back text"],
            move |run_id, _| {
                // A generator of its own for each subgraph node, over the same adapter: a mock
                // generator admits one generation at a time, and both children answer at once.
                let generator = generator.clone().reporting([Some(Usage {
                    input_tokens: Some(7),
                    output_tokens: Some(2),
                    ..Usage::default()
                })]);
                Box::new(SubgraphNode::new(
                    run_id,
                    &declaration_,
                    &child,
                    HashMap::<GenerationId, i64>::new(),
                    Box::new(move |child_run: RunId, node: &Node| {
                        assert_eq!(node.node_id(), model_id);
                        model_node(child_run, &generator, usage)
                    }),
                ))
            },
        )
    };
    let [(held, run)] = turned.usage.runs.as_slice() else {
        panic!("the session holds one run: {:#?}", turned.usage)
    };
    assert_eq!(*held, turned.run_id);
    assert_eq!(
        run.uncounted().collect::<BTreeSet<_>>(),
        expected,
        "the run's total names each node whose usage it misses"
    );
}

#[test]
fn a_record_rolls_up_two_levels_through_unconnected_usage_ports_once() {
    let middle_model = reporting(7);
    let inner_model = reporting(5);
    let (catalog, profile_id) = catalog(&middle_model);
    let adapter = middle_model.adapter().adapter_id();
    let mut declaration = Declaration::new();
    let (inner_child, inner_child_id, (_, inner_usage)) =
        child(&mut declaration, &catalog, profile_id, adapter, true);
    let (middle_id, (a, a_usage)) = declaration
        .build(&catalog, profile_id, spec("middle"), |pipeline| {
            let own = model(
                pipeline,
                adapter,
                "model",
                (INGRESS, EGRESS),
                Some("model usage"),
            );
            let inner = pipeline
                .subgraph("inner", NodeRole::Transform, inner_child_id)
                .expect("the child is Planned");
            let into = pipeline.input::<modality::Text>(&inner, INGRESS, TEXT);
            pipeline.expose(&into, "inner text", capacity(), OverloadPolicy::Pause);
            let out = pipeline.output::<modality::Text>(&inner, EGRESS, TEXT);
            pipeline.expose(&out, "inner reply", capacity(), OverloadPolicy::Pause);
            let _unconnected =
                pipeline.output::<modality::Structured>(&inner, USAGE_PORT, OPTIONAL_RECORDS);
            own
        })
        .expect("the profile is declared");
    let validated = declaration.validate(middle_id).expect("valid");
    let middle = declaration.plan(validated, &catalog).expect("planned");
    let (root_id, ()) = declaration
        .build(&catalog, profile_id, spec("root"), |pipeline| {
            let outer = pipeline
                .subgraph("outer", NodeRole::Transform, middle_id)
                .expect("the middle is Planned");
            for name in [INGRESS, "inner text"] {
                let into = pipeline.input::<modality::Text>(&outer, name, TEXT);
                pipeline.expose(&into, name, capacity(), OverloadPolicy::Pause);
            }
            for name in [EGRESS, "inner reply"] {
                let out = pipeline.output::<modality::Text>(&outer, name, TEXT);
                pipeline.expose(&out, name, capacity(), OverloadPolicy::Pause);
            }
            let _unconnected =
                pipeline.output::<modality::Structured>(&outer, USAGE_PORT, OPTIONAL_RECORDS);
        })
        .expect("the profile is declared");
    let validated = declaration.validate(root_id).expect("valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the outer node's ports match the middle's exposures");
    assert!(
        uncounted_usage(&declaration, planned.pipeline_id()).is_empty(),
        "every level has a `usage` port to pass the usage on"
    );

    let turned = {
        let declaration_ = declaration.clone();
        one_turn(
            &declaration,
            &planned,
            &[INGRESS, "inner text"],
            move |run_id, _| {
                let declaration_ = declaration_.clone();
                let inner_child = inner_child.clone();
                let middle_model = middle_model.clone();
                let inner_model = inner_model.clone();
                Box::new(SubgraphNode::new(
                    run_id,
                    &declaration_.clone(),
                    &middle,
                    HashMap::<GenerationId, i64>::new(),
                    Box::new(move |middle_run: RunId, node: &Node| {
                        if node.node_id() == a {
                            return model_node(middle_run, &middle_model, a_usage);
                        }
                        let inner_model = inner_model.clone();
                        Box::new(SubgraphNode::new(
                            middle_run,
                            &declaration_,
                            &inner_child,
                            HashMap::<GenerationId, i64>::new(),
                            Box::new(move |inner_run: RunId, _: &Node| {
                                model_node(inner_run, &inner_model, inner_usage)
                            }),
                        ))
                    }),
                ))
            },
        )
    };

    assert!(
        turned.delivered().is_empty(),
        "no exposure of the root carries usage: {:#?}",
        turned.received
    );
    let [(held, run)] = turned.usage.runs.as_slice() else {
        panic!("the session holds one run: {:#?}", turned.usage)
    };
    assert_eq!(*held, turned.run_id);
    assert_eq!(
        run.kind(QuantityKind::InputTextTokens),
        Some(KindTotal {
            known: 12,
            unknown: 0,
            records: 2,
            partial: 0,
        }),
        "the middle model's 7 and the inner model's 5, each once: {:#?}",
        turned.usage
    );
    assert_eq!(run.uncounted().count(), 0, "{run:#?}");
    assert_eq!(turned.usage.session, *run);
}
