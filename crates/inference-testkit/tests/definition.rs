//! The A-02 text pipeline of `docs/design/inference.md`, source to mock LLM to sink, written as a
//! JSON definition (`story:declarative-graph-definitions`, 491): checked against the descriptors
//! of the testkit's mock nodes, declared, validated and planned, it reaches the same effective
//! capabilities as the same pipeline built with the typed builder, and the two declarations hold
//! the same entities.
#![cfg(feature = "llm")]

use std::num::NonZeroUsize;

use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, NodeDescriptors, NodeRole, OverloadPolicy,
    PipelineDefinition, PipelineSpec, PlannedPipeline, modality,
};
use inference_testkit::graph::descriptors::{self, TEXT};
use inference_testkit::llm::{MockConfiguration, MockGenerator};
use serde_json::json;

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

fn fixture() -> Fixture {
    let generator = MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    );
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    Fixture {
        profile,
        adapter: generator.adapter().clone(),
    }
}

fn from_definition(fixture: &Fixture) -> (Declaration, PlannedPipeline) {
    let definition = json!({
        "name": "text only",
        "schema_revision": "1",
        "mode": "turn_based",
        "profile_id": fixture.profile.profile_id(),
        "required_capabilities": ["output_streaming"],
        "nodes": [
            {"name": "source", "kind": descriptors::TEXT_SOURCE},
            {"name": "model", "kind": descriptors::MODEL},
            {"name": "sink", "kind": descriptors::TEXT_SINK}
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
        "bindings": [{"node": "model", "adapter_id": fixture.adapter.adapter_id()}]
    })
    .to_string();
    let descriptors = NodeDescriptors::of(descriptors::all()).expect("three kinds");
    let mut declaration = Declaration::counted();
    let declared = declaration
        .declare_definition(
            &PipelineDefinition::from_json(&definition).expect("the definition reads"),
            &descriptors,
            fixture,
        )
        .expect("the definition matches the testkit's descriptors");
    let validated = declaration
        .validate(declared.pipeline_id)
        .expect("source to model to sink is valid");
    let planned = declaration
        .plan(validated, fixture)
        .expect("the model is bound to an adapter that streams output");
    (declaration, planned)
}

fn from_builder(fixture: &Fixture) -> (Declaration, PlannedPipeline) {
    let spec = PipelineSpec {
        name: "text only".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let mut declaration = Declaration::counted();
    let (pipeline_id, ()) = declaration
        .build(fixture, fixture.profile.profile_id(), spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let emitted = pipeline.output::<modality::Text>(&source, "text", TEXT);
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
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
                .bind(&model, fixture.adapter.adapter_id())
                .expect("the catalog holds the mock LLM's adapter");
        })
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline_id).expect("valid");
    let planned = declaration.plan(validated, fixture).expect("planned");
    (declaration, planned)
}

#[test]
fn the_a02_pipeline_from_a_json_definition_plans_to_the_builders_effective_capabilities() {
    let fixture = fixture();
    let (defined, defined_plan) = from_definition(&fixture);
    let (built, built_plan) = from_builder(&fixture);
    assert_eq!(
        defined_plan.effective_capabilities(),
        built_plan.effective_capabilities()
    );
    assert_eq!(defined_plan.downgrades(), built_plan.downgrades());
    assert!(
        defined_plan
            .effective_capabilities()
            .contains(&Capability::OutputStreaming)
    );
    assert_eq!(
        defined, built,
        "the definition and the builder declare the same entities under the same identities"
    );
}
