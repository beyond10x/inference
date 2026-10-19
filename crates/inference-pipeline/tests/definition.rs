//! Declarative graph definitions checked against node descriptors before anything is declared
//! (`story:declarative-graph-definitions`, `docs/design/inference.md` 491).
//!
//! A JSON definition naming its nodes by kind declares the same entities the typed builder
//! declares for the same pipeline. A definition naming an unknown kind, a port its descriptor
//! lacks, an edge leaving an input or entering an output, or an edge across modalities is refused
//! with a finding naming the node and the port, and the declaration holds nothing it did not hold
//! before. `DeclareNodeDescriptor` publishes a descriptor once per kind.

use std::num::NonZeroUsize;

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, Modality, ModelId,
};
use inference_core::policy::{
    DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind, ProfileNotFound,
};
use inference_pipeline::nodes::descriptors;
use inference_pipeline::{
    AdapterNotFound, Catalog, Declaration, DeclareNodeDescriptor, DefinitionDefect,
    DefinitionError, DefinitionFinding, Delivery, Essentiality, InteractionMode,
    NodeDescriptorDeclared, NodeDescriptorExists, NodeDescriptors, NodeKind, NodeRole,
    OverloadPolicy, PipelineDefinition, PipelineSpec, PortDescriptor, PortDirection, PortTraits,
    modality,
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

/// A source of text, a model from text to text needing `output_streaming`, a sink of text and an
/// audio sink, published under the kinds `source`, `model`, `sink` and `speaker`.
fn descriptors() -> NodeDescriptors {
    let text = |name: &str, direction| PortDescriptor::new(name, direction, Modality::Text, TEXT);
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
            kind: NodeKind::new("speaker"),
            role: NodeRole::Sink,
            requires: Vec::new(),
            ports: vec![PortDescriptor::new(
                "audio",
                PortDirection::Input,
                Modality::Audio,
                TEXT,
            )],
        },
    ])
    .expect("four kinds")
}

/// The definition of source to model to sink, as JSON, with the model bound.
fn text_pipeline(known: &Known) -> Value {
    json!({
        "name": "text only",
        "schema_revision": "1",
        "mode": "turn_based",
        "profile_id": known.profile.profile_id(),
        "required_capabilities": ["output_streaming"],
        "nodes": [
            {"name": "source", "kind": "source"},
            {"name": "model", "kind": "model"},
            {"name": "sink", "kind": "sink"}
        ],
        "edges": [
            {
                "from": {"node": "source", "port": "text"},
                "to": {"node": "model", "port": "prompt"},
                "capacity": 4,
                "overload": "pause"
            },
            {
                "from": {"node": "model", "port": "answer"},
                "to": {"node": "sink", "port": "text"},
                "capacity": 16,
                "overload": "fail"
            }
        ],
        "bindings": [{"node": "model", "adapter_id": known.adapter.adapter_id()}]
    })
}

fn definition(json: &Value) -> PipelineDefinition {
    PipelineDefinition::from_json(&json.to_string()).expect("the definition reads")
}

/// Declares `json` into `declaration`, which must refuse it, and checks that it holds nothing new.
fn refused(json: &Value, known: &Known) -> DefinitionError {
    let mut declaration = Declaration::counted();
    let before = declaration.clone();
    let error = declaration
        .declare_definition(&definition(json), &descriptors(), known)
        .expect_err("the definition is refused");
    assert_eq!(
        declaration, before,
        "a refused definition declares nothing and takes no identity"
    );
    error
}

fn finding(defect: DefinitionDefect, node: &str, port: Option<&str>) -> DefinitionFinding {
    DefinitionFinding {
        defect,
        node: node.to_owned(),
        port: port.map(str::to_owned),
    }
}

#[test]
fn a_definition_declares_what_the_builder_declares_and_validates_and_plans() {
    let known = known();
    let mut declared = Declaration::counted();
    let result = declared
        .declare_definition(&definition(&text_pipeline(&known)), &descriptors(), &known)
        .expect("the definition matches its descriptors");

    let mut built = Declaration::counted();
    let (pipeline_id, ()) = built
        .build(
            &known,
            known.profile.profile_id(),
            PipelineSpec {
                name: "text only".to_owned(),
                schema_revision: "1".to_owned(),
                mode: InteractionMode::TurnBased,
                required_capabilities: vec![Capability::OutputStreaming],
            },
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                let emitted = pipeline.output::<modality::Text>(&source, "text", TEXT);
                let model =
                    pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
                let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
                let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
                let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
                let collected = pipeline.input::<modality::Text>(&sink, "text", TEXT);
                pipeline.connect(
                    &emitted,
                    &prompt,
                    NonZeroUsize::new(4).expect("4"),
                    OverloadPolicy::Pause,
                );
                pipeline.connect(
                    &answer,
                    &collected,
                    NonZeroUsize::new(16).expect("16"),
                    OverloadPolicy::Fail,
                );
                pipeline
                    .bind(&model, known.adapter.adapter_id())
                    .expect("the adapter is known");
            },
        )
        .expect("the profile is known");

    assert_eq!(result.pipeline_id, pipeline_id);
    assert_eq!(
        declared, built,
        "the definition declares the builder's entities, in its order, under its identities"
    );
    assert_eq!(
        result
            .nodes
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["source", "model", "sink"]
    );
    assert_eq!(
        result
            .node("model")
            .and_then(|node_id| declared.node(node_id).map(|node| node.requires().to_vec())),
        Some(vec![Capability::OutputStreaming]),
        "the role and requirements are the descriptor's"
    );
    let validated = declared
        .validate(pipeline_id)
        .expect("source to model to sink is valid");
    let planned = declared
        .plan(validated, &known)
        .expect("the model is bound to an adapter that streams output");
    assert_eq!(
        planned.effective_capabilities(),
        [Capability::OutputStreaming]
    );
}

#[test]
fn an_unknown_kind_is_refused_naming_the_node() {
    let known = known();
    let mut json = text_pipeline(&known);
    json["nodes"][1]["kind"] = json!("llm");
    assert_eq!(
        refused(&json, &known),
        DefinitionError::Rejected(vec![finding(DefinitionDefect::UnknownKind, "model", None)]),
        "the edges and the binding of a node of unknown kind are not judged again"
    );
}

#[test]
fn a_port_the_descriptor_lacks_is_refused_naming_the_node_and_the_port() {
    let known = known();
    let mut json = text_pipeline(&known);
    json["edges"][1]["from"]["port"] = json!("reply");
    assert_eq!(
        refused(&json, &known),
        DefinitionError::Rejected(vec![finding(
            DefinitionDefect::UnknownPort,
            "model",
            Some("reply")
        )])
    );
}

#[test]
fn an_edge_across_modalities_is_refused_naming_the_node_and_the_port() {
    let known = known();
    let mut json = text_pipeline(&known);
    json["nodes"][2] = json!({"name": "sink", "kind": "speaker"});
    json["edges"][1]["to"]["port"] = json!("audio");
    assert_eq!(
        refused(&json, &known),
        DefinitionError::Rejected(vec![finding(
            DefinitionDefect::ModalityMismatch,
            "model",
            Some("answer")
        )])
    );
}

#[test]
fn an_edge_leaving_an_input_or_entering_an_output_is_refused() {
    let known = known();
    let mut json = text_pipeline(&known);
    json["edges"][0] = json!({
        "from": {"node": "model", "port": "prompt"},
        "to": {"node": "source", "port": "text"},
        "capacity": 4,
        "overload": "pause"
    });
    assert_eq!(
        refused(&json, &known),
        DefinitionError::Rejected(vec![
            finding(DefinitionDefect::EdgeFromInput, "model", Some("prompt")),
            finding(DefinitionDefect::EdgeIntoOutput, "source", Some("text")),
        ])
    );
}

#[test]
fn every_finding_is_named_at_once_and_unknown_names_are_findings() {
    let known = known();
    let mut json = text_pipeline(&known);
    json["nodes"]
        .as_array_mut()
        .expect("nodes")
        .push(json!({"name": "sink", "kind": "sink"}));
    json["edges"][0]["from"]["node"] = json!("mic");
    json["bindings"][0]["node"] = json!("llm");
    json["exposures"] = json!([{
        "name": "reply",
        "port": {"node": "sink", "port": "speaker"},
        "capacity": 1,
        "overload": "pause"
    }]);
    json["feedback_loops"] = json!([{
        "from": {"node": "sink", "port": "text"},
        "to": {"node": "model", "port": "prompt"},
        "max_iterations": 2
    }]);
    assert_eq!(
        refused(&json, &known),
        DefinitionError::Rejected(vec![
            finding(DefinitionDefect::DuplicateNode, "sink", None),
            finding(DefinitionDefect::UnknownNode, "mic", Some("text")),
            finding(DefinitionDefect::UnknownPort, "sink", Some("speaker")),
            finding(DefinitionDefect::UnknownEdge, "sink", Some("text")),
            finding(DefinitionDefect::UnknownNode, "llm", None),
        ])
    );
}

#[test]
fn an_unknown_profile_or_adapter_is_refused_after_the_check_and_declares_nothing() {
    let known = known();
    let mut json = text_pipeline(&known);
    let adapter_id = AdapterId::from(Uuid::from_u128(5));
    json["bindings"][0]["adapter_id"] = json!(adapter_id);
    assert_eq!(
        refused(&json, &known),
        DefinitionError::UnknownAdapter(AdapterNotFound { adapter_id })
    );
    let profile_id = ProfileId::from(Uuid::from_u128(6));
    json["profile_id"] = json!(profile_id);
    assert_eq!(
        refused(&json, &known),
        DefinitionError::UnknownProfile(ProfileNotFound { profile_id })
    );
    json["nodes"][0]["kind"] = json!("mystery");
    assert!(
        matches!(refused(&json, &known), DefinitionError::Rejected(_)),
        "the check answers before the catalog"
    );
}

#[test]
fn a_definition_that_is_not_of_the_shape_is_not_read() {
    let known = known();
    let mut json = text_pipeline(&known);
    json["edges"][0]["capacity"] = json!(0);
    assert!(
        PipelineDefinition::from_json(&json.to_string()).is_err(),
        "an unbounded edge"
    );
    let mut json = text_pipeline(&known);
    json["nodes"][0]["ports"] = json!([]);
    assert!(
        PipelineDefinition::from_json(&json.to_string()).is_err(),
        "a node restating its ports"
    );
    assert!(PipelineDefinition::from_json("{").is_err());
}

#[test]
fn a_descriptor_is_declared_once_per_kind() {
    let mut descriptors = NodeDescriptors::new();
    assert_eq!(
        descriptors.declare(descriptors::vad()),
        Ok(NodeDescriptorDeclared {
            kind: NodeKind::new("vad"),
            role: NodeRole::Policy,
        })
    );
    let before = descriptors.clone();
    assert_eq!(
        descriptors.declare(descriptors::vad()),
        Err(NodeDescriptorExists {
            kind: NodeKind::new("vad"),
        })
    );
    assert_eq!(descriptors, before, "a refused declaration changes nothing");
    let all = NodeDescriptors::of(descriptors::all()).expect("four kinds");
    assert_eq!(
        all.iter()
            .map(|descriptor| descriptor.kind().as_str().to_owned())
            .collect::<Vec<_>>(),
        ["vad", "turn_policy", "tool_authority", "tool_executor"]
    );
    let authority = all
        .descriptor(&NodeKind::new(descriptors::TOOL_AUTHORITY))
        .expect("declared");
    assert_eq!(
        authority.port("rejected").map(|port| port.essentiality),
        Some(Essentiality::Optional),
        "the authority's rejections are an optional output"
    );
}

#[test]
fn the_descriptor_and_definition_names_serialize_as_the_ess_names() {
    assert_eq!(
        serde_json::to_value(descriptors::turn_policy()).expect("serializes"),
        json!({
            "kind": "turn_policy",
            "role": "policy",
            "requires": [],
            "ports": [
                {
                    "name": "activity", "direction": "input", "modality": "Control",
                    "essentiality": "essential", "delivery": "reliable", "incremental": false,
                    "end_marker_required": true, "provisional_allowed": false
                },
                {
                    "name": "turns", "direction": "output", "modality": "Control",
                    "essentiality": "essential", "delivery": "reliable", "incremental": false,
                    "end_marker_required": true, "provisional_allowed": false
                }
            ]
        })
    );
    assert_eq!(
        serde_json::to_value(DefinitionDefect::ModalityMismatch).expect("serializes"),
        json!("modality_mismatch")
    );
}
