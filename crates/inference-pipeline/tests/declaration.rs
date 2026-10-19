//! The declaration half of `ess/domains/graph.yaml`: the five declaration commands over a
//! [`Declaration`], each refusing what graph.yaml refuses and creating nothing, and the typed
//! builder, which yields the same declaration as the commands for the same input.

use std::num::NonZeroUsize;

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, Modality, ModelId,
};
use inference_core::policy::{
    DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind, ProfileNotFound,
};
use inference_pipeline::{
    AdapterNotFound, AddNode, AddNodeError, AddPort, BindNode, BindNodeError, Catalog,
    ConnectPorts, ConnectPortsError, Declaration, DeclarePipeline, Delivery, EdgeDefect,
    Essentiality, IncompatibleEdge, InteractionMode, NodeId, NodeNotFound, NodeRole,
    OverloadPolicy, PipelineId, PipelineNotFound, PipelineSpec, PipelineState, PortDirection,
    PortId, PortNotFound, PortTraits, UnboundedEdge, modality,
};
use serde_json::json;
use uuid::Uuid;

/// One declared profile and one declared adapter, as a store would hold them.
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
        kind: ProfileKind::SelfHostedLan,
        egress: Egress::InternalAllowlist,
        admitted: vec![Locality::SameProcess, Locality::LocalWorker],
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

fn unknown<T: From<Uuid>>() -> T {
    T::from(Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_dead_beef))
}

fn declare_pipeline(known: &Known) -> DeclarePipeline {
    DeclarePipeline {
        name: "voice".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        profile_id: known.profile.profile_id(),
        required_capabilities: vec![Capability::OutputStreaming],
    }
}

fn add_port(node_id: NodeId, name: &str, direction: PortDirection) -> AddPort {
    AddPort {
        node_id,
        name: name.to_owned(),
        direction,
        modality: Modality::Text,
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    }
}

fn traits() -> PortTraits {
    PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    }
}

/// A pipeline with an LLM node and a sink node, declared through the commands.
fn through_the_commands(known: &Known) -> (Declaration, PipelineId, PortId, PortId) {
    let mut declaration = Declaration::counted();
    let pipeline_id = declaration
        .declare_pipeline(declare_pipeline(known), known)
        .expect("the profile is declared")
        .pipeline_id;
    let llm = declaration
        .add_node(AddNode {
            pipeline_id,
            name: "llm".to_owned(),
            role: NodeRole::Model,
            requires: vec![Capability::OutputStreaming],
        })
        .expect("the pipeline is declared")
        .node_id;
    let reply = declaration
        .add_port(add_port(llm, "reply", PortDirection::Output))
        .expect("the node exists")
        .port_id;
    let sink = declaration
        .add_node(AddNode {
            pipeline_id,
            name: "subtitles".to_owned(),
            role: NodeRole::Sink,
            requires: Vec::new(),
        })
        .expect("the pipeline is declared")
        .node_id;
    let text = declaration
        .add_port(add_port(sink, "text", PortDirection::Input))
        .expect("the node exists")
        .port_id;
    declaration
        .connect_ports(ConnectPorts {
            pipeline_id,
            from_port: reply,
            to_port: text,
            capacity: 8,
            overload: OverloadPolicy::Pause,
        })
        .expect("a bounded edge from an output port");
    declaration
        .bind_node(
            BindNode {
                pipeline_id,
                node_id: llm,
                adapter_id: known.adapter.adapter_id(),
            },
            known,
        )
        .expect("the pipeline, node and adapter exist");
    (declaration, pipeline_id, reply, text)
}

#[test]
fn the_builder_and_the_commands_yield_the_same_declaration() {
    let known = known();
    let (commands, pipeline_id, reply, text) = through_the_commands(&known);

    let mut built = Declaration::counted();
    let (built_id, (built_reply, built_text)) = built.build(
        &known.profile,
        PipelineSpec {
            name: "voice".to_owned(),
            schema_revision: "1".to_owned(),
            mode: InteractionMode::TurnBased,
            required_capabilities: vec![Capability::OutputStreaming],
        },
        |pipeline| {
            let llm = pipeline.node("llm", NodeRole::Model, vec![Capability::OutputStreaming]);
            let reply = pipeline.output::<modality::Text>(&llm, "reply", traits());
            let sink = pipeline.node("subtitles", NodeRole::Sink, Vec::new());
            let text = pipeline.input::<modality::Text>(&sink, "text", traits());
            pipeline.connect(
                &reply,
                &text,
                NonZeroUsize::new(8).expect("non-zero"),
                OverloadPolicy::Pause,
            );
            pipeline.bind(&llm, &known.adapter);
            (reply.id(), text.id())
        },
    );

    assert_eq!(built, commands);
    assert_eq!(
        (built_id, built_reply, built_text),
        (pipeline_id, reply, text)
    );
    assert_eq!(built.pipelines().count(), 1);
    assert_eq!(built.nodes().count(), 2);
    assert_eq!(built.ports().count(), 2);
    assert_eq!(built.edges().count(), 1);
    assert_eq!(built.bindings().count(), 1);
}

#[test]
fn a_declared_pipeline_holds_the_input_and_no_capabilities_planned_yet() {
    let known = known();
    let mut declaration = Declaration::counted();
    let event = declaration
        .declare_pipeline(declare_pipeline(&known), &known)
        .expect("the profile is declared");

    assert_eq!(
        event.pipeline_id.to_string(),
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(event.name, "voice");
    let pipeline = declaration
        .pipeline(event.pipeline_id)
        .expect("the pipeline is stored");
    assert_eq!(pipeline.name(), "voice");
    assert_eq!(pipeline.schema_revision(), "1");
    assert_eq!(pipeline.mode(), InteractionMode::TurnBased);
    assert_eq!(pipeline.profile_id(), known.profile.profile_id());
    assert_eq!(
        pipeline.required_capabilities(),
        [Capability::OutputStreaming]
    );
    assert_eq!(pipeline.effective_capabilities(), None);
    assert_eq!(pipeline.downgrades(), None);
    assert_eq!(pipeline.state(), PipelineState::Declared);
}

#[test]
fn every_entity_carries_the_fields_its_command_set() {
    let known = known();
    let (declaration, pipeline_id, reply, text) = through_the_commands(&known);

    let llm = declaration.nodes().next().expect("the first node");
    assert_eq!(llm.pipeline_id(), pipeline_id);
    assert_eq!(llm.name(), "llm");
    assert_eq!(llm.role(), NodeRole::Model);
    assert_eq!(llm.requires(), [Capability::OutputStreaming]);

    let port = declaration.port(reply).expect("the reply port");
    assert_eq!(port.node_id(), llm.node_id());
    assert_eq!(port.name(), "reply");
    assert_eq!(port.direction(), PortDirection::Output);
    assert_eq!(port.modality(), Modality::Text);
    assert_eq!(port.essentiality(), Essentiality::Essential);
    assert_eq!(port.delivery(), Delivery::Reliable);
    assert!(port.incremental());
    assert!(port.end_marker_required());
    assert!(!port.provisional_allowed());

    let edge = declaration.edges().next().expect("the edge");
    assert_eq!(edge.pipeline_id(), pipeline_id);
    assert_eq!(edge.from_port(), reply);
    assert_eq!(edge.to_port(), text);
    assert_eq!(edge.capacity().get(), 8);
    assert_eq!(edge.overload(), OverloadPolicy::Pause);

    let binding = declaration.bindings().next().expect("the binding");
    assert_eq!(binding.pipeline_id(), pipeline_id);
    assert_eq!(binding.node_id(), llm.node_id());
    assert_eq!(binding.adapter_id(), known.adapter.adapter_id());
}

#[test]
fn an_unknown_profile_declares_no_pipeline_and_takes_no_identity() {
    let known = known();
    let mut declaration = Declaration::counted();
    let before = declaration.clone();
    let profile_id = unknown::<ProfileId>();

    assert_eq!(
        declaration.declare_pipeline(
            DeclarePipeline {
                profile_id,
                ..declare_pipeline(&known)
            },
            &known
        ),
        Err(ProfileNotFound { profile_id })
    );
    assert_eq!(declaration, before);
}

#[test]
fn add_node_and_add_port_refuse_an_unknown_identity_and_create_nothing() {
    let known = known();
    let (mut declaration, ..) = through_the_commands(&known);
    let before = declaration.clone();
    let pipeline_id = unknown::<PipelineId>();
    let node_id = unknown::<NodeId>();

    assert_eq!(
        declaration.add_node(AddNode {
            pipeline_id,
            name: "n".to_owned(),
            role: NodeRole::Transform,
            requires: Vec::new(),
        }),
        Err(AddNodeError::UnknownPipeline(PipelineNotFound {
            pipeline_id
        }))
    );
    assert_eq!(
        declaration.add_port(add_port(node_id, "p", PortDirection::Input)),
        Err(NodeNotFound { node_id })
    );
    assert_eq!(declaration, before);
}

#[test]
fn connect_ports_decides_its_refusals_in_the_order_graph_yaml_lists_them() {
    let known = known();
    let (mut declaration, pipeline_id, reply, text) = through_the_commands(&known);
    let before = declaration.clone();
    let connect = |pipeline_id, from_port, capacity| ConnectPorts {
        pipeline_id,
        from_port,
        to_port: text,
        capacity,
        overload: OverloadPolicy::Drop,
    };
    let nowhere = unknown::<PipelineId>();
    let dangling = unknown::<PortId>();

    assert_eq!(
        declaration.connect_ports(connect(nowhere, dangling, 0)),
        Err(ConnectPortsError::UnknownPipeline(PipelineNotFound {
            pipeline_id: nowhere
        }))
    );
    assert_eq!(
        declaration.connect_ports(connect(pipeline_id, dangling, 0)),
        Err(ConnectPortsError::UnknownSource(PortNotFound {
            port_id: dangling
        }))
    );
    for submitted in [0, -1, i64::MIN] {
        assert_eq!(
            declaration.connect_ports(connect(pipeline_id, text, submitted)),
            Err(ConnectPortsError::Unbounded(UnboundedEdge { submitted }))
        );
    }
    assert_eq!(
        declaration.connect_ports(connect(pipeline_id, text, 1)),
        Err(ConnectPortsError::SourceNotOutput(IncompatibleEdge {
            from_port: text,
            to_port: text,
            defect: EdgeDefect::SourceNotOutput,
        }))
    );
    assert_eq!(declaration, before);

    let event = declaration
        .connect_ports(ConnectPorts {
            to_port: dangling,
            ..connect(pipeline_id, reply, 1)
        })
        .expect("the sink port is validation's to check, not the command's");
    assert_eq!(event.to_port, dangling);
}

#[test]
fn bind_node_decides_its_refusals_in_the_order_graph_yaml_lists_them() {
    let known = known();
    let (mut declaration, pipeline_id, ..) = through_the_commands(&known);
    let before = declaration.clone();
    let node_id = declaration.nodes().next().expect("a node").node_id();
    let nowhere = unknown::<PipelineId>();
    let no_node = unknown::<NodeId>();
    let no_adapter = unknown::<AdapterId>();
    let bind = |pipeline_id, node_id, adapter_id| BindNode {
        pipeline_id,
        node_id,
        adapter_id,
    };

    assert_eq!(
        declaration.bind_node(bind(nowhere, no_node, no_adapter), &known),
        Err(BindNodeError::UnknownPipeline(PipelineNotFound {
            pipeline_id: nowhere
        }))
    );
    assert_eq!(
        declaration.bind_node(bind(pipeline_id, no_node, no_adapter), &known),
        Err(BindNodeError::UnknownNode(NodeNotFound {
            node_id: no_node
        }))
    );
    assert_eq!(
        declaration.bind_node(bind(pipeline_id, node_id, no_adapter), &known),
        Err(BindNodeError::UnknownAdapter(AdapterNotFound {
            adapter_id: no_adapter
        }))
    );
    assert_eq!(declaration, before);
}

#[test]
fn values_serialize_as_the_ess_names() {
    let to = |value: serde_json::Result<serde_json::Value>| value.expect("serializes");
    assert_eq!(
        to(serde_json::to_value(NodeRole::ALL)),
        json!([
            "source",
            "model",
            "transform",
            "policy",
            "tool",
            "fan_out",
            "fan_in",
            "sink"
        ])
    );
    assert_eq!(
        to(serde_json::to_value(PortDirection::ALL)),
        json!(["input", "output"])
    );
    assert_eq!(
        to(serde_json::to_value(Essentiality::ALL)),
        json!(["essential", "optional", "control_only"])
    );
    assert_eq!(
        to(serde_json::to_value(Delivery::ALL)),
        json!(["reliable", "droppable", "coalescable"])
    );
    assert_eq!(
        to(serde_json::to_value(InteractionMode::ALL)),
        json!(["batch", "turn_based", "continuous", "duplex"])
    );
    assert_eq!(
        to(serde_json::to_value(OverloadPolicy::ALL)),
        json!([
            "pause",
            "cancel",
            "drop",
            "coalesce",
            "supersede",
            "fail",
            "drop_stale_epochs"
        ])
    );
    assert_eq!(
        to(serde_json::to_value(EdgeDefect::SourceNotOutput)),
        json!("source_not_output")
    );
    assert_eq!(
        to(serde_json::to_value(PipelineState::ALL)),
        json!(["Declared", "Validated", "Planned"])
    );
    let port = PortId::from(Uuid::from_u128(1));
    assert_eq!(
        to(serde_json::to_value(IncompatibleEdge {
            from_port: port,
            to_port: port,
            defect: EdgeDefect::SourceNotOutput,
        })),
        json!({
            "from_port": "00000000-0000-0000-0000-000000000001",
            "to_port": "00000000-0000-0000-0000-000000000001",
            "defect": "source_not_output"
        })
    );
    assert_eq!(
        to(serde_json::to_value(UnboundedEdge { submitted: -3 })),
        json!({ "submitted": -3 })
    );
}
