//! Subgraph nodes (`story:subgraph-declaration`, `docs/design/inference.md` 544): a Planned
//! pipeline declared as a node of another, its ports checked against the child's exposures at
//! planning.
//!
//! The child is the A-02 text pipeline with exposures: the model node alone, its `prompt` exposed
//! as the ingress `user text` and its `answer` as the egress `reply`. A parent of a source, a
//! subgraph node over that child and a sink plans, taking the child's effective capabilities as
//! the node's, with no binding of its own. A subgraph node lacking a port for the child's egress,
//! or carrying one of another modality, or carrying a port no exposure matches, is refused with a
//! finding naming the node and the exposure or port. `AddSubgraphNode` refuses an unknown parent,
//! an unknown child, a parent no longer Declared and a child not Planned, in that order.
//!
//! The usage roll-up (`story:usage-subgraph-rollup`): a subgraph node's structured output port
//! `usage` matches every usage egress of its child that no port of the node names, and the node
//! forwards them on it; an egress a port names crosses to that port alone. A usage egress with
//! neither is refused, as is a `usage` port, or one of another modality, that forwards nothing.

use std::num::NonZeroUsize;

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, Modality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    AddNode, AddPort, AddSubgraphNode, AddSubgraphNodeError, Catalog, Declaration, Delivery,
    Essentiality, ExposureId, InteractionMode, LocalityDenied, NodeId, NodeRole, OverloadPolicy,
    PipelineId, PipelineNotFound, PipelineNotReady, PipelineSpec, PipelineState, PlanPipelineError,
    PortDirection, PortId, PortTraits, SubgraphInterfaceMismatch, SubgraphNodeAdded, modality,
};
use serde_json::json;
use uuid::Uuid;

struct Known {
    profile: DeploymentProfile,
    adapter: Adapter,
}

impl Catalog for Known {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        (self.adapter.adapter_id() == adapter_id).then_some(&self.adapter)
    }
}

fn known() -> Known {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![Locality::SameProcess],
    })
    .expect("a row of the table");
    let (adapter, _) = Adapter::declare(DeclareAdapter {
        model_id: ModelId::from(Uuid::from_u128(9)),
        backend: "mock".to_owned(),
        contract_revision: "llm/1".to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![Capability::OutputStreaming],
        max_concurrency: 1,
    })
    .expect("a positive concurrency");
    Known { profile, adapter }
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    }
}

/// The child: the model alone between the ingress `user text` and the egress `reply`, bound,
/// validated and planned. Answers the child and its two exposures.
fn child(declaration: &mut Declaration, known: &Known) -> (PipelineId, ExposureId, ExposureId) {
    let (pipeline_id, (ingress, egress)) = declaration
        .build(
            known,
            known.profile.profile_id(),
            spec("exposed model"),
            |pipeline| {
                let model =
                    pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
                let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
                let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
                let ingress = pipeline.expose(
                    &prompt,
                    "user text",
                    NonZeroUsize::new(4).expect("4"),
                    OverloadPolicy::Pause,
                );
                let egress = pipeline.expose(
                    &answer,
                    "reply",
                    NonZeroUsize::new(16).expect("16"),
                    OverloadPolicy::Fail,
                );
                pipeline
                    .bind(&model, known.adapter.adapter_id())
                    .expect("the adapter is known");
                (ingress.id(), egress.id())
            },
        )
        .expect("the profile is known");
    let validated = declaration.validate(pipeline_id).expect("valid");
    let _planned = declaration.plan(validated, known).expect("planned");
    (pipeline_id, ingress, egress)
}

/// What the subgraph node of a parent carries: its ports as name, direction and modality.
type Ports<'a> = &'a [(&'a str, PortDirection, Modality)];

/// The parent: a source of text, a subgraph node over `child` with `ports`, and a sink of text,
/// the source feeding the node's input named `user text` and the node's output named `reply`
/// feeding the sink, where the node has them; validated. Answers the parent, the node and the
/// node's ports by name.
fn parent(
    declaration: &mut Declaration,
    known: &Known,
    child: PipelineId,
    ports: Ports<'_>,
) -> (PipelineId, NodeId, Vec<(String, PortId)>) {
    let pipeline_id = declaration
        .declare_pipeline(
            inference_pipeline::DeclarePipeline {
                name: "parent".to_owned(),
                schema_revision: "1".to_owned(),
                mode: InteractionMode::TurnBased,
                profile_id: known.profile.profile_id(),
                required_capabilities: vec![Capability::OutputStreaming],
            },
            known,
        )
        .expect("the profile is known")
        .pipeline_id;
    let mut add = |name: &str, role| {
        declaration
            .add_node(AddNode {
                pipeline_id,
                name: name.to_owned(),
                role,
                requires: Vec::new(),
            })
            .expect("declared")
            .node_id
    };
    let source = add("source", NodeRole::Source);
    let sink = add("sink", NodeRole::Sink);
    let node = declaration
        .add_subgraph_node(AddSubgraphNode {
            pipeline_id,
            name: "frontend".to_owned(),
            role: NodeRole::Transform,
            subgraph_id: child,
        })
        .expect("the child is Planned")
        .node_id;
    let mut port = |node_id, name: &str, direction, modality, essentiality| {
        declaration
            .add_port(AddPort {
                pipeline_id,
                node_id,
                name: name.to_owned(),
                direction,
                modality,
                essentiality,
                delivery: TEXT.delivery,
                incremental: TEXT.incremental,
                end_marker_required: TEXT.end_marker_required,
                provisional_allowed: TEXT.provisional_allowed,
            })
            .expect("declared")
            .port_id
    };
    let essential = Essentiality::Essential;
    let emitted = port(
        source,
        "text",
        PortDirection::Output,
        Modality::Text,
        essential,
    );
    let collected = port(
        sink,
        "text",
        PortDirection::Input,
        Modality::Text,
        essential,
    );
    let node_ports: Vec<(String, PortId, PortDirection, Modality)> = ports
        .iter()
        .map(|(name, direction, modality)| {
            let port_id = port(node, name, *direction, *modality, Essentiality::Optional);
            ((*name).to_owned(), port_id, *direction, *modality)
        })
        .collect();
    let text_port = |name: &str, wanted: PortDirection| {
        node_ports
            .iter()
            .find(|(port, _, direction, modality)| {
                port == name && *direction == wanted && *modality == Modality::Text
            })
            .map(|(_, port_id, _, _)| *port_id)
    };
    let mut connect = |from, to| {
        declaration
            .connect_ports(inference_pipeline::ConnectPorts {
                pipeline_id,
                from_port: from,
                to_port: to,
                capacity: 4,
                overload: OverloadPolicy::Pause,
            })
            .expect("declared");
    };
    let input = text_port("user text", PortDirection::Input);
    let output = text_port("reply", PortDirection::Output);
    if let Some(input) = input {
        connect(emitted, input);
    }
    match (input, output) {
        (Some(_), Some(output)) => connect(output, collected),
        _ => connect(emitted, collected),
    }
    let node_ports = node_ports
        .into_iter()
        .map(|(name, port_id, _, _)| (name, port_id))
        .collect();
    (pipeline_id, node, node_ports)
}

const MATCHING: Ports<'static> = &[
    ("user text", PortDirection::Input, Modality::Text),
    ("reply", PortDirection::Output, Modality::Text),
];

#[test]
fn a_parent_with_a_subgraph_node_over_the_exposed_text_pipeline_plans_with_the_childs_capabilities()
{
    let known = known();
    let mut declaration = Declaration::counted();
    let (child, _, _) = child(&mut declaration, &known);
    let (parent, node, _) = parent(&mut declaration, &known, child, MATCHING);
    assert_eq!(
        declaration.node(node).map(|node| (
            node.subgraph_id(),
            node.role(),
            node.requires().is_empty()
        )),
        Some((Some(child), NodeRole::Transform, true)),
        "the node stands for the child, takes the role its declarer named and requires nothing"
    );
    let validated = declaration
        .validate(parent)
        .expect("source, node, sink is valid");
    let planned = declaration
        .plan(validated, &known)
        .expect("the node's ports match the child's exposures, and it needs no binding");
    assert_eq!(
        planned.effective_capabilities(),
        [Capability::OutputStreaming],
        "the node contributes the child's effective capabilities, the weakest on the path"
    );
    assert_eq!(
        declaration
            .pipeline(parent)
            .map(inference_pipeline::Pipeline::state),
        Some(PipelineState::Planned)
    );
}

/// Plans the parent and answers the refusal, checking that it stays Validated.
fn refused(
    ports: Ports<'_>,
) -> (
    SubgraphInterfaceMismatch,
    NodeId,
    Vec<(String, PortId)>,
    ExposureId,
    ExposureId,
) {
    let known = known();
    let mut declaration = Declaration::counted();
    let (child, ingress, egress) = child(&mut declaration, &known);
    let (parent, node, node_ports) = parent(&mut declaration, &known, child, ports);
    let validated = declaration.validate(parent).expect("valid");
    let Err(PlanPipelineError::InterfaceMismatch(mismatch)) = declaration.plan(validated, &known)
    else {
        panic!("the parent is refused for its subgraph node's interface");
    };
    assert_eq!(
        declaration
            .pipeline(parent)
            .map(inference_pipeline::Pipeline::state),
        Some(PipelineState::Validated),
        "the parent stays Validated"
    );
    (mismatch, node, node_ports, ingress, egress)
}

#[test]
fn a_subgraph_node_lacking_a_port_for_the_childs_egress_is_refused_naming_the_node_and_exposure() {
    let (mismatch, node, _, _, egress) =
        refused(&[("user text", PortDirection::Input, Modality::Text)]);
    assert_eq!(
        mismatch,
        SubgraphInterfaceMismatch {
            node_id: node,
            port_id: None,
            exposure_id: Some(egress),
        }
    );
}

#[test]
fn a_subgraph_node_port_of_another_modality_is_refused_naming_the_node_the_port_and_exposure() {
    let (mismatch, node, ports, _, egress) = refused(&[
        ("user text", PortDirection::Input, Modality::Text),
        ("reply", PortDirection::Output, Modality::Audio),
    ]);
    assert_eq!(
        mismatch,
        SubgraphInterfaceMismatch {
            node_id: node,
            port_id: Some(ports[1].1),
            exposure_id: Some(egress),
        }
    );
}

#[test]
fn a_subgraph_node_port_no_exposure_matches_is_refused_naming_the_port() {
    let (mismatch, node, ports, _, _) = refused(&[
        ("user text", PortDirection::Input, Modality::Text),
        ("reply", PortDirection::Output, Modality::Text),
        ("status", PortDirection::Output, Modality::Control),
    ]);
    assert_eq!(
        mismatch,
        SubgraphInterfaceMismatch {
            node_id: node,
            port_id: Some(ports[2].1),
            exposure_id: None,
        }
    );
}

#[test]
fn a_port_of_the_wrong_direction_is_refused_naming_the_port_and_exposure() {
    let (mismatch, node, ports, ingress, _) = refused(&[
        ("user text", PortDirection::Output, Modality::Text),
        ("reply", PortDirection::Output, Modality::Text),
    ]);
    assert_eq!(
        mismatch,
        SubgraphInterfaceMismatch {
            node_id: node,
            port_id: Some(ports[0].1),
            exposure_id: Some(ingress),
        }
    );
}

#[test]
fn add_subgraph_node_refuses_in_order_and_a_refusal_creates_nothing() {
    let known = known();
    let mut declaration = Declaration::counted();
    let (child, _, _) = child(&mut declaration, &known);
    let parent = declaration
        .declare_pipeline(
            inference_pipeline::DeclarePipeline {
                name: "parent".to_owned(),
                schema_revision: "1".to_owned(),
                mode: InteractionMode::TurnBased,
                profile_id: known.profile.profile_id(),
                required_capabilities: Vec::new(),
            },
            &known,
        )
        .expect("known")
        .pipeline_id;
    let unknown = PipelineId::from(Uuid::from_u128(77));
    let input = |pipeline_id, subgraph_id| AddSubgraphNode {
        pipeline_id,
        name: "frontend".to_owned(),
        role: NodeRole::Transform,
        subgraph_id,
    };
    let before = declaration.clone();
    assert_eq!(
        declaration.add_subgraph_node(input(unknown, unknown)),
        Err(AddSubgraphNodeError::UnknownPipeline(PipelineNotFound {
            pipeline_id: unknown,
        }))
    );
    assert_eq!(
        declaration.add_subgraph_node(input(parent, unknown)),
        Err(AddSubgraphNodeError::UnknownSubgraph(PipelineNotFound {
            pipeline_id: unknown,
        }))
    );
    assert_eq!(
        declaration.add_subgraph_node(input(child, parent)),
        Err(AddSubgraphNodeError::NotDeclared(PipelineNotReady {
            pipeline_id: child,
            required: PipelineState::Declared,
        })),
        "a Planned child gains no node"
    );
    assert_eq!(
        declaration.add_subgraph_node(input(parent, parent)),
        Err(AddSubgraphNodeError::SubgraphNotPlanned(PipelineNotReady {
            pipeline_id: parent,
            required: PipelineState::Planned,
        })),
        "a pipeline cannot contain itself: a Declared one is not Planned"
    );
    assert_eq!(declaration, before, "a refusal creates nothing");
    let added = declaration
        .add_subgraph_node(input(parent, child))
        .expect("a Declared parent over a Planned child");
    assert_eq!(
        added,
        SubgraphNodeAdded {
            node_id: added.node_id,
            pipeline_id: parent,
            subgraph_id: child,
            name: "frontend".to_owned(),
        }
    );
}

#[test]
fn the_builder_adds_a_subgraph_node_only_over_a_planned_child() {
    let known = known();
    let mut declaration = Declaration::counted();
    let (child, _, _) = child(&mut declaration, &known);
    let (declared_child, ()) = declaration
        .build(&known, known.profile.profile_id(), spec("declared"), |_| ())
        .expect("known");
    let (parent, (node, refused)) = declaration
        .build(
            &known,
            known.profile.profile_id(),
            spec("parent"),
            |pipeline| {
                let node = pipeline
                    .subgraph("frontend", NodeRole::Transform, child)
                    .expect("the child is Planned");
                pipeline.input::<modality::Text>(&node, "user text", TEXT);
                pipeline.output::<modality::Text>(&node, "reply", TEXT);
                let refused = pipeline
                    .subgraph("again", NodeRole::Transform, declared_child)
                    .err();
                (node.id(), refused)
            },
        )
        .expect("known");
    assert_eq!(
        refused,
        Some(AddSubgraphNodeError::SubgraphNotPlanned(PipelineNotReady {
            pipeline_id: declared_child,
            required: PipelineState::Planned,
        }))
    );
    assert_eq!(
        declaration
            .node(node)
            .and_then(inference_pipeline::Node::subgraph_id),
        Some(child)
    );
    assert_eq!(
        declaration
            .nodes()
            .filter(|node| node.pipeline_id() == parent)
            .count(),
        1,
        "the refused node was not added"
    );
}

#[test]
fn the_subgraph_names_serialize_as_the_ess_names() {
    let node_id = NodeId::from(Uuid::from_u128(1));
    let exposure_id = ExposureId::from(Uuid::from_u128(2));
    assert_eq!(
        serde_json::to_value(SubgraphInterfaceMismatch {
            node_id,
            port_id: None,
            exposure_id: Some(exposure_id),
        })
        .expect("serializes"),
        json!({
            "node_id": "00000000-0000-0000-0000-000000000001",
            "port_id": null,
            "exposure_id": "00000000-0000-0000-0000-000000000002"
        })
    );
}

/// Two profiles, one admitting the same process only and one the external provider as well, and
/// an adapter at each locality.
struct Mixed {
    offline: DeploymentProfile,
    hybrid: DeploymentProfile,
    local: Adapter,
    hosted: Adapter,
}

impl Catalog for Mixed {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        [&self.offline, &self.hybrid]
            .into_iter()
            .find(|profile| profile.profile_id() == profile_id)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        [&self.local, &self.hosted]
            .into_iter()
            .find(|adapter| adapter.adapter_id() == adapter_id)
    }
}

#[test]
fn a_binding_nested_two_subgraphs_deep_is_refused_under_the_parents_profile_naming_the_inner_node()
{
    let profile = |kind: ProfileKind, admitted| {
        DeploymentProfile::declare(DeclareProfile {
            kind,
            egress: kind.egress(),
            admitted,
        })
        .expect("a row of the table")
        .0
    };
    let adapter = |locality, execution_form| {
        Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "model".to_owned(),
            contract_revision: "llm/1".to_owned(),
            execution_form,
            locality,
            capabilities: vec![Capability::OutputStreaming],
            max_concurrency: 1,
        })
        .expect("a positive concurrency")
        .0
    };
    let known = Mixed {
        offline: profile(ProfileKind::OfflineStrict, vec![Locality::SameProcess]),
        hybrid: profile(
            ProfileKind::HybridOptIn,
            vec![Locality::SameProcess, Locality::ExternalProvider],
        ),
        local: adapter(Locality::SameProcess, ExecutionForm::InProcess),
        hosted: adapter(Locality::ExternalProvider, ExecutionForm::HostedService),
    };
    let capacity = NonZeroUsize::new(4).expect("4");
    let mut declaration = Declaration::new();
    let planned = |declaration: &mut Declaration, pipeline_id| {
        let validated = declaration.validate(pipeline_id).expect("valid");
        let _planned = declaration
            .plan(validated, &known)
            .expect("planned under the hybrid profile");
    };

    let (innermost, hosted_model) = declaration
        .build(
            &known,
            known.hybrid.profile_id(),
            spec("hosted"),
            |pipeline| {
                let model = pipeline.node("model", NodeRole::Model, Vec::new());
                let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
                let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
                pipeline.expose(&prompt, "user text", capacity, OverloadPolicy::Pause);
                pipeline.expose(&answer, "reply", capacity, OverloadPolicy::Pause);
                pipeline
                    .bind(&model, known.hosted.adapter_id())
                    .expect("the adapter is known");
                model.id()
            },
        )
        .expect("the profile is known");
    planned(&mut declaration, innermost);
    let (middle, ()) = declaration
        .build(
            &known,
            known.hybrid.profile_id(),
            spec("middle"),
            |pipeline| {
                let inner = pipeline
                    .subgraph("inner", NodeRole::Transform, innermost)
                    .expect("the innermost is Planned");
                let asked = pipeline.input::<modality::Text>(&inner, "user text", TEXT);
                let replied = pipeline.output::<modality::Text>(&inner, "reply", TEXT);
                pipeline.expose(&asked, "user text", capacity, OverloadPolicy::Pause);
                pipeline.expose(&replied, "reply", capacity, OverloadPolicy::Pause);
            },
        )
        .expect("the profile is known");
    planned(&mut declaration, middle);
    let (parent, ()) = declaration
        .build(
            &known,
            known.offline.profile_id(),
            spec("offline"),
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                let local = pipeline.node("local", NodeRole::Transform, Vec::new());
                let frontend = pipeline
                    .subgraph("frontend", NodeRole::Transform, middle)
                    .expect("the middle is Planned");
                let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
                let emitted = pipeline.output::<modality::Text>(&source, "text", TEXT);
                let taken = pipeline.input::<modality::Text>(&local, "in", TEXT);
                let passed = pipeline.output::<modality::Text>(&local, "out", TEXT);
                let asked = pipeline.input::<modality::Text>(&frontend, "user text", TEXT);
                let replied = pipeline.output::<modality::Text>(&frontend, "reply", TEXT);
                let collected = pipeline.input::<modality::Text>(&sink, "text", TEXT);
                pipeline.connect(&emitted, &taken, capacity, OverloadPolicy::Pause);
                pipeline.connect(&passed, &asked, capacity, OverloadPolicy::Pause);
                pipeline.connect(&replied, &collected, capacity, OverloadPolicy::Pause);
                pipeline
                    .bind(&local, known.local.adapter_id())
                    .expect("the adapter is known");
            },
        )
        .expect("the profile is known");
    let validated = declaration.validate(parent).expect("valid");

    assert_eq!(
        declaration.plan(validated, &known),
        Err(PlanPipelineError::LocalityDenied(LocalityDenied {
            node_id: hosted_model,
            adapter_id: known.hosted.adapter_id(),
            locality: Locality::ExternalProvider,
            profile_id: known.offline.profile_id(),
        })),
        "the parent's offline profile decides for every binding its subgraphs reach"
    );
    assert_eq!(
        declaration
            .pipeline(parent)
            .map(inference_pipeline::Pipeline::state),
        Some(PipelineState::Validated),
        "nothing moved"
    );
}

/// The usage roll-up's child (`story:usage-subgraph-rollup`): one model per name of `usage`, the
/// first between the ingress `user text` and the egress `reply`, the model `i` after it between
/// `user text {i}` and `reply {i}`, each with a structured output port `usage` exposed as the
/// egress `usage[i]`; bound, validated and planned. Answers the child and the usage exposures in
/// that order.
fn usage_child(
    declaration: &mut Declaration,
    known: &Known,
    usage: &[&str],
) -> (PipelineId, Vec<ExposureId>) {
    let records = PortTraits {
        incremental: false,
        ..TEXT
    };
    let capacity = NonZeroUsize::new(4).expect("4");
    let (pipeline_id, exposures) = declaration
        .build(
            known,
            known.profile.profile_id(),
            spec("models with usage"),
            |pipeline| {
                let mut exposures = Vec::new();
                for (i, name) in usage.iter().enumerate() {
                    let (ingress, egress) = match i {
                        0 => ("user text".to_owned(), "reply".to_owned()),
                        _ => (format!("user text {i}"), format!("reply {i}")),
                    };
                    let model = pipeline.node(
                        format!("model {i}"),
                        NodeRole::Model,
                        vec![Capability::OutputStreaming],
                    );
                    let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
                    let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
                    let used = pipeline.output::<modality::Structured>(&model, "usage", records);
                    pipeline.expose(&prompt, ingress, capacity, OverloadPolicy::Pause);
                    pipeline.expose(&answer, egress, capacity, OverloadPolicy::Pause);
                    exposures.push(
                        pipeline
                            .expose(&used, *name, capacity, OverloadPolicy::Pause)
                            .id(),
                    );
                    pipeline
                        .bind(&model, known.adapter.adapter_id())
                        .expect("the adapter is known");
                }
                exposures
            },
        )
        .expect("the profile is known");
    let validated = declaration.validate(pipeline_id).expect("valid");
    let _planned = declaration.plan(validated, known).expect("planned");
    (pipeline_id, exposures)
}

/// The text ports of a subgraph node over a [`usage_child`] of `models` models, then `more`.
fn usage_ports(models: usize, more: &[(&str, PortDirection, Modality)]) -> Vec<NamedPort> {
    let mut ports: Vec<NamedPort> = MATCHING
        .iter()
        .map(|(name, direction, modality)| ((*name).to_owned(), *direction, *modality))
        .collect();
    for i in 1..models {
        ports.push((
            format!("user text {i}"),
            PortDirection::Input,
            Modality::Text,
        ));
        ports.push((format!("reply {i}"), PortDirection::Output, Modality::Text));
    }
    ports.extend(
        more.iter()
            .map(|(name, direction, modality)| ((*name).to_owned(), *direction, *modality)),
    );
    ports
}

type NamedPort = (String, PortDirection, Modality);

/// A parent over a [`usage_child`], validated, and what its cases read of it.
struct UsageParent {
    known: Known,
    declaration: Declaration,
    parent: PipelineId,
    node: NodeId,
    ports: Vec<(String, PortId)>,
    exposures: Vec<ExposureId>,
}

impl UsageParent {
    /// A parent over a [`usage_child`] naming `usage`, its subgraph node carrying `ports`.
    fn new(usage: &[&str], ports: &[NamedPort]) -> Self {
        let known = known();
        let mut declaration = Declaration::counted();
        let (child, exposures) = usage_child(&mut declaration, &known, usage);
        let ports: Vec<(&str, PortDirection, Modality)> = ports
            .iter()
            .map(|(name, direction, modality)| (name.as_str(), *direction, *modality))
            .collect();
        let (parent, node, ports) = parent(&mut declaration, &known, child, &ports);
        Self {
            known,
            declaration,
            parent,
            node,
            ports,
            exposures,
        }
    }

    /// The exposures the subgraph node forwards on its `usage` port.
    fn forwarded(&self) -> Vec<ExposureId> {
        self.declaration
            .forwarded_usage(self.node)
            .iter()
            .map(|exposure| exposure.exposure_id())
            .collect()
    }

    /// Plans the parent.
    fn plan(&mut self) -> Result<(), PlanPipelineError> {
        let validated = self.declaration.validate(self.parent).expect("valid");
        self.declaration
            .plan(validated, &self.known)
            .map(|_planned| ())
    }

    /// Plans the parent and answers its interface refusal.
    fn refused(&mut self) -> SubgraphInterfaceMismatch {
        let Err(PlanPipelineError::InterfaceMismatch(mismatch)) = self.plan() else {
            panic!("the parent is refused for its subgraph node's interface");
        };
        mismatch
    }
}

const USAGE: (&str, PortDirection, Modality) =
    ("usage", PortDirection::Output, Modality::Structured);

#[test]
fn a_usage_port_forwards_every_usage_egress_of_the_child_no_port_names_and_the_parent_plans() {
    let mut fixture = UsageParent::new(&["first usage", "second usage"], &usage_ports(2, &[USAGE]));
    assert_eq!(inference_pipeline::USAGE_PORT, "usage");
    fixture.plan().expect(
        "the node's `usage` port matches both usage egresses of the child, which no port names",
    );
    assert_eq!(
        fixture.forwarded(),
        fixture.exposures,
        "the node forwards both, in the child's exposure order"
    );
}

#[test]
fn a_usage_egress_a_port_of_the_node_names_crosses_by_name_and_is_not_forwarded_again() {
    let mut fixture = UsageParent::new(
        &["usage", "second usage"],
        &usage_ports(
            2,
            &[
                USAGE,
                ("second usage", PortDirection::Output, Modality::Structured),
            ],
        ),
    );
    fixture
        .plan()
        .expect("each usage egress has the port of its name");
    assert!(
        fixture.forwarded().is_empty(),
        "an egress crosses to the port of its name only, so no record is forwarded twice"
    );
}

#[test]
fn a_usage_port_and_the_usage_egress_of_its_name_forward_the_other_usage_egresses_too() {
    let mut fixture = UsageParent::new(&["usage", "second usage"], &usage_ports(2, &[USAGE]));
    fixture
        .plan()
        .expect("`usage` by name, `second usage` forwarded on it");
    assert_eq!(fixture.forwarded(), fixture.exposures[1..]);
}

#[test]
fn without_a_usage_port_a_usage_egress_of_the_child_is_refused_naming_it_so_no_usage_is_lost() {
    let mut fixture = UsageParent::new(&["model usage"], &usage_ports(1, &[]));
    assert!(fixture.forwarded().is_empty());
    assert_eq!(
        fixture.refused(),
        SubgraphInterfaceMismatch {
            node_id: fixture.node,
            port_id: None,
            exposure_id: Some(fixture.exposures[0]),
        }
    );
}

#[test]
fn a_usage_port_of_another_modality_forwards_nothing_and_is_refused_naming_it() {
    let mut fixture = UsageParent::new(
        &["model usage"],
        &usage_ports(1, &[("usage", PortDirection::Output, Modality::Text)]),
    );
    assert!(fixture.forwarded().is_empty());
    assert_eq!(
        fixture.refused(),
        SubgraphInterfaceMismatch {
            node_id: fixture.node,
            port_id: Some(fixture.ports[2].1),
            exposure_id: None,
        }
    );
}

#[test]
fn a_usage_port_over_a_child_without_a_usage_egress_is_refused_naming_the_port() {
    let known = known();
    let mut declaration = Declaration::counted();
    let (child, _, _) = child(&mut declaration, &known);
    let ports = [MATCHING[0], MATCHING[1], USAGE];
    let (parent, node, node_ports) = parent(&mut declaration, &known, child, &ports);
    assert!(declaration.forwarded_usage(node).is_empty());
    let validated = declaration.validate(parent).expect("valid");
    assert_eq!(
        declaration.plan(validated, &known).map(|_planned| ()),
        Err(PlanPipelineError::InterfaceMismatch(
            SubgraphInterfaceMismatch {
                node_id: node,
                port_id: Some(node_ports[2].1),
                exposure_id: None,
            }
        ))
    );
}
