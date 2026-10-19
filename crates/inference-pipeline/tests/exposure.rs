//! `inference.graph.ExposePort` and the exposures of a pipeline (`story:pipeline-exposed-ports`,
//! design line 511: named source and sink interfaces exposed to the application).
//!
//! The command exposes a port of a Declared pipeline under a name with finite buffering, and
//! refuses an unknown pipeline, an unknown port, an unbounded capacity and a pipeline that is no
//! longer Declared, in that order, creating nothing then. Validation counts an exposed input port
//! as fed and an exposed output port as consumed (check 6), finds a port of another pipeline
//! missing (check 1) and two exposures of one name (`duplicate_exposure_name`). The typed builder
//! declares the same exposure as the command.

use std::num::NonZeroUsize;

use inference_core::model::{Adapter, AdapterId, Capability, Locality, Modality};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    AddNode, AddPort, Catalog, Declaration, DeclarePipeline, Delivery, Essentiality, ExposePort,
    ExposePortError, ExposureId, GraphDefect, GraphFinding, IncompatibleGraph, InteractionMode,
    NodeId, NodeRole, OverloadPolicy, PipelineId, PipelineNotFound, PipelineNotReady, PipelineSpec,
    PipelineState, PipelineValidated, PortDirection, PortExposed, PortId, PortNotFound, PortTraits,
    UnboundedEdge, ValidatePipeline, ValidatePipelineError, direction, modality,
};
use serde_json::json;
use uuid::Uuid;

struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

fn profiles() -> Profiles {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![Locality::SameProcess],
    })
    .expect("a row of the table");
    Profiles(profile)
}

const TRAITS: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

/// One declaration holding pipelines declared through the commands.
struct Graph {
    profiles: Profiles,
    declaration: Declaration,
}

impl Graph {
    fn new() -> Self {
        Self {
            profiles: profiles(),
            declaration: Declaration::counted(),
        }
    }

    fn pipeline(&mut self) -> PipelineId {
        self.declaration
            .declare_pipeline(
                DeclarePipeline {
                    name: "text only".to_owned(),
                    schema_revision: "1".to_owned(),
                    mode: InteractionMode::TurnBased,
                    profile_id: self.profiles.0.profile_id(),
                    required_capabilities: Vec::new(),
                },
                &self.profiles,
            )
            .expect("the profile is declared")
            .pipeline_id
    }

    fn node(&mut self, pipeline_id: PipelineId, name: &str, role: NodeRole) -> NodeId {
        self.declaration
            .add_node(AddNode {
                pipeline_id,
                name: name.to_owned(),
                role,
                requires: Vec::new(),
            })
            .expect("the pipeline is declared")
            .node_id
    }

    fn port(
        &mut self,
        pipeline_id: PipelineId,
        node_id: NodeId,
        name: &str,
        direction: PortDirection,
    ) -> PortId {
        self.declaration
            .add_port(AddPort {
                pipeline_id,
                node_id,
                name: name.to_owned(),
                direction,
                modality: Modality::Text,
                essentiality: TRAITS.essentiality,
                delivery: TRAITS.delivery,
                incremental: TRAITS.incremental,
                end_marker_required: TRAITS.end_marker_required,
                provisional_allowed: TRAITS.provisional_allowed,
            })
            .expect("the pipeline and the node are declared")
            .port_id
    }

    fn expose(
        &mut self,
        pipeline_id: PipelineId,
        port_id: PortId,
        name: &str,
        capacity: i64,
    ) -> Result<PortExposed, ExposePortError> {
        self.declaration.expose_port(ExposePort {
            pipeline_id,
            port_id,
            name: name.to_owned(),
            capacity,
            overload: OverloadPolicy::Pause,
        })
    }

    fn validate(
        &mut self,
        pipeline_id: PipelineId,
    ) -> Result<PipelineValidated, ValidatePipelineError> {
        self.declaration
            .validate_pipeline(ValidatePipeline { pipeline_id })
    }
}

/// A model node with a text input port `prompt` and a text output port `answer`, alone in its
/// pipeline: no source feeds it and no sink consumes it.
struct Model {
    pipeline_id: PipelineId,
    node_id: NodeId,
    prompt: PortId,
    answer: PortId,
}

fn model(graph: &mut Graph) -> Model {
    let pipeline_id = graph.pipeline();
    let node_id = graph.node(pipeline_id, "model", NodeRole::Model);
    let prompt = graph.port(pipeline_id, node_id, "prompt", PortDirection::Input);
    let answer = graph.port(pipeline_id, node_id, "answer", PortDirection::Output);
    Model {
        pipeline_id,
        node_id,
        prompt,
        answer,
    }
}

fn counted(n: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | n)
}

#[test]
fn an_exposed_port_is_an_exposure_of_its_pipeline_with_its_name_capacity_and_overload() {
    let mut graph = Graph::new();
    let model = model(&mut graph);

    let ingress = graph
        .expose(model.pipeline_id, model.prompt, "user text", 4)
        .expect("the pipeline is declared and the port exists");
    let egress = graph
        .declaration
        .expose_port(ExposePort {
            pipeline_id: model.pipeline_id,
            port_id: model.answer,
            name: "reply".to_owned(),
            capacity: 16,
            overload: OverloadPolicy::Fail,
        })
        .expect("the pipeline is declared and the port exists");

    assert_eq!(
        ingress,
        PortExposed {
            exposure_id: ExposureId::from(counted(5)),
            pipeline_id: model.pipeline_id,
            port_id: model.prompt,
            name: "user text".to_owned(),
        },
        "the exposure takes the next counted identity"
    );
    assert_eq!(egress.exposure_id, ExposureId::from(counted(6)));
    let exposures: Vec<_> = graph
        .declaration
        .exposures()
        .map(|exposure| {
            (
                exposure.exposure_id(),
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
        [
            (
                ingress.exposure_id,
                model.pipeline_id,
                model.prompt,
                "user text".to_owned(),
                4,
                OverloadPolicy::Pause,
            ),
            (
                egress.exposure_id,
                model.pipeline_id,
                model.answer,
                "reply".to_owned(),
                16,
                OverloadPolicy::Fail,
            ),
        ]
    );
    assert_eq!(
        graph
            .declaration
            .exposure(egress.exposure_id)
            .map(|exposure| exposure.name().to_owned()),
        Some("reply".to_owned())
    );
}

#[test]
fn expose_port_refuses_in_order_and_a_refusal_creates_nothing_and_takes_no_identity() {
    let mut graph = Graph::new();
    let model = model(&mut graph);
    let unknown_pipeline = PipelineId::from(Uuid::from_u128(77));
    let unknown_port = PortId::from(Uuid::from_u128(78));
    let before = graph.declaration.clone();

    assert_eq!(
        graph.expose(unknown_pipeline, unknown_port, "x", 0),
        Err(ExposePortError::UnknownPipeline(PipelineNotFound {
            pipeline_id: unknown_pipeline,
        })),
        "the pipeline is decided first"
    );
    assert_eq!(
        graph.expose(model.pipeline_id, unknown_port, "x", 0),
        Err(ExposePortError::UnknownPort(PortNotFound {
            port_id: unknown_port,
        })),
        "then the port"
    );
    for capacity in [0, -1] {
        assert_eq!(
            graph.expose(model.pipeline_id, model.prompt, "x", capacity),
            Err(ExposePortError::Unbounded(UnboundedEdge {
                submitted: capacity,
            })),
            "then the capacity {capacity}"
        );
    }
    assert_eq!(graph.declaration, before, "a refusal creates nothing");

    graph
        .expose(model.pipeline_id, model.prompt, "user text", 1)
        .expect("the pipeline is declared");
    graph
        .expose(model.pipeline_id, model.answer, "reply", 1)
        .expect("the pipeline is declared");
    graph
        .validate(model.pipeline_id)
        .expect("a model between an ingress and an egress is valid");
    let validated = graph.declaration.clone();
    assert_eq!(
        graph.expose(model.pipeline_id, model.prompt, "again", 1),
        Err(ExposePortError::NotDeclared(PipelineNotReady {
            pipeline_id: model.pipeline_id,
            required: PipelineState::Declared,
        })),
        "a validated pipeline is exposed no further"
    );
    assert_eq!(graph.declaration, validated);
}

#[test]
fn check_six_counts_an_ingress_as_fed_and_an_egress_as_consumed() {
    let mut graph = Graph::new();
    let alone = model(&mut graph);
    assert_eq!(
        graph.validate(alone.pipeline_id),
        Err(ValidatePipelineError::Rejected(IncompatibleGraph {
            findings: vec![GraphFinding {
                defect: GraphDefect::UnreachableMandatoryOutput,
                node_id: Some(alone.node_id),
                port_id: Some(alone.answer),
            }],
        })),
        "unexposed, the model's essential output feeds nothing"
    );

    let exposed = model(&mut graph);
    graph
        .expose(exposed.pipeline_id, exposed.prompt, "user text", 4)
        .expect("declared");
    graph
        .expose(exposed.pipeline_id, exposed.answer, "reply", 4)
        .expect("declared");
    assert_eq!(
        graph.validate(exposed.pipeline_id),
        Ok(PipelineValidated {
            pipeline_id: exposed.pipeline_id,
        }),
        "with no source or sink node, the exposures are the source and sink paths"
    );
}

#[test]
fn a_sink_reached_from_an_ingress_alone_is_reached() {
    let mut graph = Graph::new();
    let pipeline_id = graph.pipeline();
    let model = graph.node(pipeline_id, "model", NodeRole::Model);
    let sink = graph.node(pipeline_id, "sink", NodeRole::Sink);
    let prompt = graph.port(pipeline_id, model, "prompt", PortDirection::Input);
    let answer = graph.port(pipeline_id, model, "answer", PortDirection::Output);
    let collected = graph.port(pipeline_id, sink, "text", PortDirection::Input);
    graph
        .declaration
        .connect_ports(inference_pipeline::ConnectPorts {
            pipeline_id,
            from_port: answer,
            to_port: collected,
            capacity: 4,
            overload: OverloadPolicy::Pause,
        })
        .expect("declared");
    assert_eq!(
        graph.declaration.findings(pipeline_id),
        [GraphFinding {
            defect: GraphDefect::UnreachableMandatoryOutput,
            node_id: Some(sink),
            port_id: None,
        }],
        "no source reaches the sink"
    );
    graph
        .expose(pipeline_id, prompt, "user text", 4)
        .expect("declared");
    assert_eq!(graph.declaration.findings(pipeline_id), []);
}

#[test]
fn an_exposed_port_of_another_pipeline_is_missing_from_the_pipeline() {
    let mut graph = Graph::new();
    let exposed = model(&mut graph);
    let other = model(&mut graph);
    graph
        .expose(exposed.pipeline_id, exposed.prompt, "user text", 4)
        .expect("declared");
    graph
        .expose(exposed.pipeline_id, exposed.answer, "reply", 4)
        .expect("declared");
    graph
        .expose(exposed.pipeline_id, other.prompt, "elsewhere", 4)
        .expect("a port of another pipeline is not refused by the command");
    assert_eq!(
        graph.declaration.findings(exposed.pipeline_id),
        [GraphFinding {
            defect: GraphDefect::MissingNodeOrPort,
            node_id: None,
            port_id: Some(other.prompt),
        }]
    );
}

#[test]
fn two_exposures_of_one_name_are_a_finding_naming_the_later_one() {
    let mut graph = Graph::new();
    let model = model(&mut graph);
    graph
        .expose(model.pipeline_id, model.prompt, "text", 4)
        .expect("declared");
    graph
        .expose(model.pipeline_id, model.answer, "text", 4)
        .expect("a second exposure of one name is not refused by the command");
    assert_eq!(
        graph.validate(model.pipeline_id),
        Err(ValidatePipelineError::Rejected(IncompatibleGraph {
            findings: vec![GraphFinding {
                defect: GraphDefect::DuplicateExposureName,
                node_id: Some(model.node_id),
                port_id: Some(model.answer),
            }],
        }))
    );
    assert_eq!(
        graph
            .declaration
            .pipeline(model.pipeline_id)
            .map(inference_pipeline::Pipeline::state),
        Some(PipelineState::Declared),
        "the pipeline stays Declared"
    );
}

#[test]
fn one_name_in_two_pipelines_is_no_finding() {
    let mut graph = Graph::new();
    let first = model(&mut graph);
    let second = model(&mut graph);
    for model in [&first, &second] {
        graph
            .expose(model.pipeline_id, model.prompt, "user text", 4)
            .expect("declared");
        graph
            .expose(model.pipeline_id, model.answer, "reply", 4)
            .expect("declared");
    }
    assert_eq!(graph.declaration.findings(first.pipeline_id), []);
    assert_eq!(graph.declaration.findings(second.pipeline_id), []);
}

#[test]
fn the_builder_exposes_what_the_command_exposes_with_the_ports_direction_and_modality() {
    let mut graph = Graph::new();
    let profile_id = graph.profiles.0.profile_id();
    let spec = PipelineSpec {
        name: "text only".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut built = Declaration::counted();
    let (pipeline_id, (ingress, egress)) = built
        .build(&graph.profiles, profile_id, spec, |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, Vec::new());
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", TRAITS);
            let answer = pipeline.output::<modality::Text>(&model, "answer", TRAITS);
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
            (ingress, egress)
        })
        .expect("the profile is declared");
    let _: inference_pipeline::Exposed<direction::Input, modality::Text> = ingress;
    let _: inference_pipeline::Exposed<direction::Output, modality::Text> = egress;
    assert_eq!(
        (ingress.direction(), ingress.modality()),
        (PortDirection::Input, Modality::Text)
    );
    assert_eq!(
        (egress.direction(), egress.modality()),
        (PortDirection::Output, Modality::Text)
    );

    let model = model(&mut graph);
    assert_eq!(model.pipeline_id, pipeline_id);
    let by_command = graph
        .expose(model.pipeline_id, model.prompt, "user text", 4)
        .expect("declared");
    graph
        .declaration
        .expose_port(ExposePort {
            pipeline_id: model.pipeline_id,
            port_id: model.answer,
            name: "reply".to_owned(),
            capacity: 16,
            overload: OverloadPolicy::Fail,
        })
        .expect("declared");
    assert_eq!(ingress.id(), by_command.exposure_id);
    assert_eq!(
        built, graph.declaration,
        "the builder and the commands declare the same entities with the same identities"
    );
    assert_eq!(
        built
            .validate(pipeline_id)
            .map(|validated| validated.pipeline_id()),
        Ok(pipeline_id),
        "a model between an ingress and an egress validates through the builder"
    );
    let _: Capability = Capability::OutputStreaming;
}

#[test]
fn the_exposure_names_serialize_as_the_ess_names() {
    let exposure_id = ExposureId::from(Uuid::from_u128(1));
    let pipeline_id = PipelineId::from(Uuid::from_u128(2));
    let port_id = PortId::from(Uuid::from_u128(3));
    assert_eq!(
        serde_json::to_value(GraphDefect::DuplicateExposureName).expect("serializes"),
        json!("duplicate_exposure_name")
    );
    assert_eq!(
        serde_json::to_value(ExposePort {
            pipeline_id,
            port_id,
            name: "reply".to_owned(),
            capacity: 4,
            overload: OverloadPolicy::DropStaleEpochs,
        })
        .expect("serializes"),
        json!({
            "pipeline_id": "00000000-0000-0000-0000-000000000002",
            "port_id": "00000000-0000-0000-0000-000000000003",
            "name": "reply",
            "capacity": 4,
            "overload": "drop_stale_epochs"
        })
    );
    assert_eq!(
        serde_json::to_value(PortExposed {
            exposure_id,
            pipeline_id,
            port_id,
            name: "reply".to_owned(),
        })
        .expect("serializes"),
        json!({
            "exposure_id": "00000000-0000-0000-0000-000000000001",
            "pipeline_id": "00000000-0000-0000-0000-000000000002",
            "port_id": "00000000-0000-0000-0000-000000000003",
            "name": "reply"
        })
    );
}
