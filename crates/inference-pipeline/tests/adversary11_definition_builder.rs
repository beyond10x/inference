//! Adversary cases against declarative definitions (`story:declarative-graph-definitions`): a
//! definition the check passes declares what the typed builder declares for the same pipeline,
//! exposures and feedback loops included, and a definition naming a port of its descriptor is
//! never refused for it.

use std::num::NonZeroUsize;

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, Modality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, DeclareNodeDescriptor, Delivery, Essentiality, InteractionMode,
    NodeDescriptors, NodeKind, NodeRole, OverloadPolicy, PipelineDefinition, PipelineSpec,
    PortDescriptor, PortDirection, PortTraits, modality,
};
use serde_json::{Value, json};
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

fn text(name: &str, direction: PortDirection) -> PortDescriptor {
    PortDescriptor::new(name, direction, Modality::Text, TEXT)
}

/// `source` (text out), `model` (prompt in, answer out), `sink` (text in) and `relay`, whose input
/// and output are both named `text`.
fn descriptors() -> NodeDescriptors {
    NodeDescriptors::of([
        DeclareNodeDescriptor {
            kind: NodeKind::new("source"),
            role: NodeRole::Source,
            requires: Vec::new(),
            ports: vec![text("text", PortDirection::Output)],
        },
        DeclareNodeDescriptor {
            kind: NodeKind::new("model"),
            role: NodeRole::Model,
            requires: vec![Capability::OutputStreaming],
            ports: vec![
                text("prompt", PortDirection::Input),
                text("answer", PortDirection::Output),
            ],
        },
        DeclareNodeDescriptor {
            kind: NodeKind::new("sink"),
            role: NodeRole::Sink,
            requires: Vec::new(),
            ports: vec![text("text", PortDirection::Input)],
        },
        DeclareNodeDescriptor {
            kind: NodeKind::new("relay"),
            role: NodeRole::Transform,
            requires: Vec::new(),
            ports: vec![
                text("text", PortDirection::Input),
                text("text", PortDirection::Output),
            ],
        },
    ])
    .expect("four kinds")
}

fn definition(json: &Value) -> PipelineDefinition {
    PipelineDefinition::from_json(&json.to_string()).expect("of the shape")
}

fn base(known: &Known) -> Value {
    json!({
        "name": "text only",
        "schema_revision": "1",
        "mode": "turn_based",
        "profile_id": known.profile.profile_id(),
        "nodes": [
            {"name": "source", "kind": "source"},
            {"name": "model", "kind": "model"},
            {"name": "sink", "kind": "sink"}
        ],
        "bindings": [{"node": "model", "adapter_id": known.adapter.adapter_id()}]
    })
}

#[test]
fn a_definition_edge_and_exposure_capacity_beyond_the_ess_integer_is_held_as_the_builder_holds_it()
{
    let known = known();
    let largest = usize::MAX;
    let mut json = base(&known);
    json["edges"] = json!([
        {
            "from": {"node": "source", "port": "text"},
            "to": {"node": "model", "port": "prompt"},
            "capacity": largest,
            "overload": "pause"
        },
        {
            "from": {"node": "model", "port": "answer"},
            "to": {"node": "sink", "port": "text"},
            "capacity": 1,
            "overload": "pause"
        }
    ]);
    json["exposures"] = json!([{
        "name": "reply",
        "port": {"node": "model", "port": "answer"},
        "capacity": largest,
        "overload": "pause"
    }]);
    let mut declared = Declaration::new();
    declared
        .declare_definition(&definition(&json), &descriptors(), &known)
        .expect("the check passes");

    let mut built = Declaration::new();
    built
        .build(
            &known,
            known.profile.profile_id(),
            PipelineSpec {
                name: "text only".to_owned(),
                schema_revision: "1".to_owned(),
                mode: InteractionMode::TurnBased,
                required_capabilities: Vec::new(),
            },
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                let emitted = pipeline.output::<modality::Text>(&source, "text", TEXT);
                let model =
                    pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
                let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
                let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
                pipeline.connect(&emitted, &prompt, NonZeroUsize::MAX, OverloadPolicy::Pause);
                pipeline.expose(&answer, "reply", NonZeroUsize::MAX, OverloadPolicy::Pause);
            },
        )
        .expect("the profile is known");

    let held = |declaration: &Declaration| {
        (
            declaration.edges().next().map(|edge| edge.capacity().get()),
            declaration
                .exposures()
                .next()
                .map(|exposure| exposure.capacity().get()),
        )
    };
    assert_eq!(
        held(&built),
        (Some(i64::MAX as usize), Some(i64::MAX as usize)),
        "the builder holds a capacity beyond i64::MAX as i64::MAX"
    );
    assert_eq!(
        held(&declared),
        held(&built),
        "the definition declares the capacity the builder declares for the same pipeline"
    );
}

#[test]
fn a_definition_declares_its_exposure_and_its_feedback_loop() {
    let known = known();
    let mut json = base(&known);
    json["edges"] = json!([
        {
            "from": {"node": "source", "port": "text"},
            "to": {"node": "model", "port": "prompt"},
            "capacity": 4,
            "overload": "pause"
        },
        {
            "from": {"node": "model", "port": "answer"},
            "to": {"node": "sink", "port": "text"},
            "capacity": 4,
            "overload": "pause"
        },
        {
            "from": {"node": "model", "port": "answer"},
            "to": {"node": "model", "port": "prompt"},
            "capacity": 2,
            "overload": "drop"
        }
    ]);
    json["exposures"] = json!([{
        "name": "reply",
        "port": {"node": "model", "port": "answer"},
        "capacity": 3,
        "overload": "coalesce"
    }]);
    json["feedback_loops"] = json!([{
        "from": {"node": "model", "port": "answer"},
        "to": {"node": "model", "port": "prompt"},
        "max_iterations": 5
    }]);
    let mut declared = Declaration::new();
    let result = declared
        .declare_definition(&definition(&json), &descriptors(), &known)
        .expect("the check passes");
    let answer = declared
        .ports()
        .find(|port| Some(port.node_id()) == result.node("model") && port.name() == "answer")
        .map(inference_pipeline::Port::port_id)
        .expect("the model's answer port");

    let exposures: Vec<_> = declared
        .exposures()
        .map(|exposure| {
            (
                exposure.pipeline_id(),
                exposure.port_id(),
                exposure.name().to_owned(),
                exposure.capacity().get(),
                exposure.overload(),
            )
        })
        .collect();
    assert_eq!(
        exposures,
        [(
            result.pipeline_id,
            answer,
            "reply".to_owned(),
            3,
            OverloadPolicy::Coalesce
        )],
        "the exposure of the definition is declared"
    );
    let feedback_edge = declared
        .edges()
        .nth(2)
        .map(inference_pipeline::Edge::edge_id)
        .expect("the third edge");
    let loops: Vec<_> = declared
        .feedback_loops()
        .map(|feedback| (feedback.edge_id(), feedback.max_iterations().get()))
        .collect();
    assert_eq!(
        loops,
        [(feedback_edge, 5)],
        "the feedback loop of the definition is declared on its edge"
    );
    let validated = declared
        .validate(result.pipeline_id)
        .expect("the loop through the model is bounded by its feedback loop");
    let _planned = declared
        .plan(validated, &known)
        .expect("the model is bound to an adapter that streams output");
}

#[test]
fn a_definition_edge_from_a_descriptors_output_is_not_refused_when_an_input_shares_its_name() {
    let known = known();
    let mut json = base(&known);
    json["nodes"] = json!([
        {"name": "source", "kind": "source"},
        {"name": "relay", "kind": "relay"},
        {"name": "sink", "kind": "sink"}
    ]);
    json["bindings"] = json!([]);
    json["edges"] = json!([
        {
            "from": {"node": "source", "port": "text"},
            "to": {"node": "relay", "port": "text"},
            "capacity": 4,
            "overload": "pause"
        },
        {
            "from": {"node": "relay", "port": "text"},
            "to": {"node": "sink", "port": "text"},
            "capacity": 4,
            "overload": "pause"
        }
    ]);
    let mut declared = Declaration::new();
    let answer = declared.declare_definition(&definition(&json), &descriptors(), &known);
    assert!(
        answer.is_ok(),
        "the relay's descriptor declares an output `text`, which the builder connects: {answer:?}"
    );
}
