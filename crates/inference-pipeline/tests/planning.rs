//! `inference.graph.PlanPipeline`: check 4 of §10.2 and the rule of §8.3 (A-13). A node requiring
//! a capability its bound adapter lacks is refused naming the node and the capability, a node
//! whose requirements need an adapter and has none is refused as unbound, an adapter running at a
//! locality the pipeline's profile does not admit is refused before either (A-04), an unbound
//! transform is transparent, and over random acyclic graphs the effective capabilities are the
//! intersection over the bound nodes on every required path.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    CapabilityMismatch, Catalog, Declaration, Essentiality, InteractionMode, LocalityDenied,
    NodeHandle, NodeId, NodeRole, NodeUnbound, OverloadPolicy, PipelineBuilder, PipelineId,
    PipelineNotFound, PipelinePlanned, PipelineSpec, PipelineState, PipelineStateConflict,
    PlanPipeline, PlanPipelineError, PortTraits, ValidatePipeline, ValidatePipelineError, modality,
};
use proptest::prelude::*;
use uuid::Uuid;

use Capability::{Interruptible, NativeDuplex, OutputStreaming, RevisableInput, ToolCalling};

/// The declared profile and the adapters a plan reads.
struct Known {
    profile: DeploymentProfile,
    adapters: Vec<Adapter>,
}

impl Catalog for Known {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.adapter_id() == adapter_id)
    }
}

impl Known {
    fn new(adapters: Vec<Adapter>) -> Self {
        Self::under(
            ProfileKind::OfflineStrict,
            vec![Locality::SameProcess],
            adapters,
        )
    }

    /// A profile of `kind`, with the table's egress for it, admitting `admitted`.
    fn under(kind: ProfileKind, admitted: Vec<Locality>, adapters: Vec<Adapter>) -> Self {
        let (profile, _) = DeploymentProfile::declare(DeclareProfile {
            kind,
            egress: kind.egress(),
            admitted,
        })
        .expect("a row of the table");
        Self { profile, adapters }
    }
}

/// A mock adapter claiming `capabilities`.
fn adapter(capabilities: Vec<Capability>) -> Adapter {
    adapter_at(Locality::SameProcess, capabilities)
}

/// A mock adapter running at `locality` and claiming `capabilities`.
fn adapter_at(locality: Locality, capabilities: Vec<Capability>) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::from(Uuid::from_u128(9)),
        backend: "mock".to_owned(),
        contract_revision: "llm/1".to_owned(),
        execution_form: match locality {
            Locality::SameProcess => ExecutionForm::InProcess,
            Locality::LocalWorker | Locality::SelfHostedCluster => ExecutionForm::LocalService,
            Locality::ExternalProvider => ExecutionForm::HostedService,
        },
        locality,
        capabilities,
        max_concurrency: 1,
    })
    .expect("a positive concurrency")
    .0
}

fn spec(required_capabilities: Vec<Capability>) -> PipelineSpec {
    PipelineSpec {
        name: "voice".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities,
    }
}

fn optional() -> PortTraits {
    PortTraits {
        essentiality: Essentiality::Optional,
        ..PortTraits::default()
    }
}

/// Connects each node to the next with a text edge whose two ports carry `traits`.
fn chain<'b>(
    pipeline: &mut PipelineBuilder<'_, 'b>,
    nodes: &[&NodeHandle<'b>],
    traits: PortTraits,
) {
    for pair in nodes.windows(2) {
        let out = pipeline.output::<modality::Text>(pair[0], "out", traits);
        let into = pipeline.input::<modality::Text>(pair[1], "in", traits);
        pipeline.connect(&out, &into, NonZeroUsize::MIN, OverloadPolicy::Pause);
    }
}

fn state(declaration: &Declaration, pipeline_id: PipelineId) -> PipelineState {
    declaration
        .pipeline(pipeline_id)
        .expect("the pipeline is declared")
        .state()
}

/// A source, a model requiring `requires` and bound to `bound` when given, and a sink, in a chain
/// of essential edges; validated.
fn source_model_sink(
    known: &Known,
    required: Vec<Capability>,
    requires: Vec<Capability>,
    bound: Option<usize>,
) -> (Declaration, PipelineId, NodeId) {
    let mut declaration = Declaration::counted();
    let (pipeline_id, model) = declaration
        .build(
            known,
            known.profile.profile_id(),
            spec(required),
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                let model = pipeline.node("llm", NodeRole::Model, requires);
                let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
                chain(pipeline, &[&source, &model, &sink], PortTraits::default());
                if let Some(index) = bound {
                    pipeline
                        .bind(&model, known.adapters[index].adapter_id())
                        .expect("the adapter is declared");
                }
                model.id()
            },
        )
        .expect("the profile is declared");
    assert_eq!(
        declaration.findings(pipeline_id),
        [],
        "the arrangement validates"
    );
    (declaration, pipeline_id, model)
}

#[test]
fn a_node_requiring_tool_calling_bound_to_an_adapter_without_it_is_refused_naming_both() {
    let known = Known::new(vec![adapter(vec![OutputStreaming, Interruptible])]);
    let (mut declaration, pipeline_id, model) =
        source_model_sink(&known, Vec::new(), vec![ToolCalling], Some(0));
    let validated = declaration
        .validate(pipeline_id)
        .expect("the graph is valid");

    let refused = declaration.plan(validated, &known);

    let mismatch = CapabilityMismatch {
        node_id: Some(model),
        capability: ToolCalling,
    };
    assert_eq!(refused, Err(PlanPipelineError::Mismatch(mismatch)));
    assert_eq!(
        declaration.plan_pipeline(PlanPipeline { pipeline_id }, &known),
        Err(PlanPipelineError::Mismatch(mismatch)),
        "the command by identity refuses the same"
    );
    let pipeline = declaration.pipeline(pipeline_id).expect("declared");
    assert_eq!(pipeline.state(), PipelineState::Validated, "nothing moved");
    assert_eq!(pipeline.effective_capabilities(), None);
    assert_eq!(pipeline.downgrades(), None);
}

#[test]
fn a_node_short_of_several_capabilities_names_the_first_in_declaration_order() {
    let known = Known::new(vec![adapter(vec![OutputStreaming])]);
    let (declaration, pipeline_id, model) = source_model_sink(
        &known,
        Vec::new(),
        vec![Interruptible, ToolCalling],
        Some(0),
    );
    assert_eq!(
        declaration.plan_of(pipeline_id, &known),
        Err(PlanPipelineError::Mismatch(CapabilityMismatch {
            node_id: Some(model),
            capability: ToolCalling,
        }))
    );
}

#[test]
fn a_node_requiring_tool_calling_with_no_binding_is_refused_as_unbound_naming_it() {
    let known = Known::new(vec![adapter(vec![ToolCalling])]);
    let (mut declaration, pipeline_id, model) =
        source_model_sink(&known, Vec::new(), vec![ToolCalling], None);
    declaration
        .validate_pipeline(ValidatePipeline { pipeline_id })
        .expect("the graph is valid");

    assert_eq!(
        declaration.plan_pipeline(PlanPipeline { pipeline_id }, &known),
        Err(PlanPipelineError::Unbound(NodeUnbound { node_id: model }))
    );
    assert_eq!(state(&declaration, pipeline_id), PipelineState::Validated);
}

#[test]
fn an_unbound_node_is_refused_before_a_mismatch_of_another() {
    let known = Known::new(vec![adapter(vec![OutputStreaming])]);
    let mut declaration = Declaration::counted();
    let (pipeline_id, unbound) = declaration
        .build(
            &known,
            known.profile.profile_id(),
            spec(Vec::new()),
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                let short = pipeline.node("short", NodeRole::Model, vec![ToolCalling]);
                let unbound = pipeline.node("unbound", NodeRole::Model, vec![RevisableInput]);
                let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
                chain(
                    pipeline,
                    &[&source, &short, &unbound, &sink],
                    PortTraits::default(),
                );
                pipeline
                    .bind(&short, known.adapters[0].adapter_id())
                    .expect("the adapter is declared");
                unbound.id()
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("the graph is valid");
    assert_eq!(
        declaration.plan(validated, &known),
        Err(PlanPipelineError::Unbound(NodeUnbound { node_id: unbound }))
    );
}

#[test]
fn a_capability_the_pipeline_requires_and_no_path_implements_names_no_node() {
    let known = Known::new(vec![adapter(vec![OutputStreaming, ToolCalling])]);
    let (mut declaration, pipeline_id, _) = source_model_sink(
        &known,
        vec![ToolCalling, Interruptible, NativeDuplex],
        Vec::new(),
        Some(0),
    );
    let validated = declaration
        .validate(pipeline_id)
        .expect("the graph is valid");
    assert_eq!(
        declaration.plan(validated, &known),
        Err(PlanPipelineError::Mismatch(CapabilityMismatch {
            node_id: None,
            capability: NativeDuplex,
        })),
        "the first unmet capability in the declaration order of Capability"
    );
}

#[test]
fn a_planned_pipeline_carries_the_intersection_and_no_downgrade() {
    let known = Known::new(vec![
        adapter(vec![OutputStreaming, ToolCalling, Interruptible]),
        adapter(vec![Interruptible, OutputStreaming, NativeDuplex]),
    ]);
    let mut declaration = Declaration::counted();
    let (pipeline_id, ()) = declaration
        .build(
            &known,
            known.profile.profile_id(),
            spec(vec![OutputStreaming]),
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                let model = pipeline.node("llm", NodeRole::Model, vec![ToolCalling]);
                let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
                chain(pipeline, &[&source, &model, &sink], PortTraits::default());
                pipeline
                    .bind(&model, known.adapters[0].adapter_id())
                    .expect("the adapter is declared");
                pipeline
                    .bind(&sink, known.adapters[1].adapter_id())
                    .expect("the adapter is declared");
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("the graph is valid");

    let planned = declaration.plan(validated, &known).expect("planned");

    assert_eq!(planned.pipeline_id(), pipeline_id);
    assert_eq!(
        planned.effective_capabilities(),
        [OutputStreaming, Interruptible]
    );
    assert_eq!(planned.downgrades(), []);
    let pipeline = declaration.pipeline(pipeline_id).expect("declared");
    assert_eq!(pipeline.state(), PipelineState::Planned);
    assert_eq!(
        pipeline.effective_capabilities(),
        Some([OutputStreaming, Interruptible].as_slice())
    );
    assert_eq!(pipeline.downgrades(), Some([].as_slice()));
}

#[test]
fn an_unbound_transform_between_two_bound_nodes_leaves_the_intersection_unchanged() {
    let known = Known::new(vec![
        adapter(vec![OutputStreaming, ToolCalling, Interruptible]),
        adapter(vec![ToolCalling, Interruptible, NativeDuplex]),
    ]);
    let plan = |with_transform: bool| {
        let mut declaration = Declaration::counted();
        let (pipeline_id, ()) = declaration
            .build(
                &known,
                known.profile.profile_id(),
                spec(Vec::new()),
                |pipeline| {
                    let source = pipeline.node("source", NodeRole::Source, Vec::new());
                    let first = pipeline.node("stt", NodeRole::Model, Vec::new());
                    let transform = pipeline.node("transform", NodeRole::Transform, Vec::new());
                    let second = pipeline.node("llm", NodeRole::Model, Vec::new());
                    let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
                    if with_transform {
                        chain(
                            pipeline,
                            &[&source, &first, &transform, &second, &sink],
                            PortTraits::default(),
                        );
                    } else {
                        chain(
                            pipeline,
                            &[&source, &first, &second, &sink],
                            PortTraits::default(),
                        );
                        chain(pipeline, &[&source, &transform, &sink], optional());
                    }
                    pipeline
                        .bind(&first, known.adapters[0].adapter_id())
                        .expect("the adapter is declared");
                    pipeline
                        .bind(&second, known.adapters[1].adapter_id())
                        .expect("the adapter is declared");
                },
            )
            .expect("the profile is declared");
        let validated = declaration
            .validate(pipeline_id)
            .expect("the graph is valid");
        declaration
            .plan(validated, &known)
            .expect("planned")
            .effective_capabilities()
            .to_vec()
    };

    assert_eq!(plan(true), [ToolCalling, Interruptible]);
    assert_eq!(plan(true), plan(false));
}

#[test]
fn a_node_off_every_required_path_contributes_nothing() {
    let known = Known::new(vec![adapter(Vec::new())]);
    let mut declaration = Declaration::counted();
    let (pipeline_id, ()) = declaration
        .build(
            &known,
            known.profile.profile_id(),
            spec(Vec::new()),
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                let side = pipeline.node("side", NodeRole::Model, Vec::new());
                let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
                chain(pipeline, &[&source, &sink], PortTraits::default());
                chain(pipeline, &[&source, &side, &sink], optional());
                pipeline
                    .bind(&side, known.adapters[0].adapter_id())
                    .expect("the adapter is declared");
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("the graph is valid");
    assert_eq!(
        declaration
            .plan(validated, &known)
            .expect("planned")
            .effective_capabilities(),
        Capability::ALL
    );
}

#[test]
fn an_adapter_the_catalog_does_not_hold_implements_nothing() {
    let known = Known::new(Vec::new());
    let elsewhere = adapter(Capability::ALL.to_vec());
    let binding_time = Known {
        profile: known.profile.clone(),
        adapters: vec![elsewhere.clone()],
    };
    let mut declaration = Declaration::counted();
    let (pipeline_id, ()) = declaration
        .build(
            &binding_time,
            known.profile.profile_id(),
            spec(Vec::new()),
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                let model = pipeline.node("llm", NodeRole::Model, Vec::new());
                let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
                chain(pipeline, &[&source, &model, &sink], PortTraits::default());
                pipeline
                    .bind(&model, elsewhere.adapter_id())
                    .expect("the adapter is declared");
            },
        )
        .expect("the profile is declared");
    assert_eq!(declaration.plan_of(pipeline_id, &known), Ok(Vec::new()));
}

#[test]
fn an_empty_pipeline_plans_with_no_effective_capability() {
    let known = Known::new(Vec::new());
    let mut declaration = Declaration::counted();
    let (pipeline_id, ()) = declaration
        .build(
            &known,
            known.profile.profile_id(),
            spec(vec![OutputStreaming]),
            |_| (),
        )
        .expect("the profile is declared");
    declaration
        .validate_pipeline(ValidatePipeline { pipeline_id })
        .expect("an empty graph is valid");

    assert_eq!(
        declaration.plan_pipeline(PlanPipeline { pipeline_id }, &known),
        Ok(PipelinePlanned { pipeline_id }),
        "no bound adapter falls short of what the pipeline requires"
    );
    let pipeline = declaration.pipeline(pipeline_id).expect("declared");
    assert_eq!(pipeline.state(), PipelineState::Planned);
    assert_eq!(pipeline.effective_capabilities(), Some([].as_slice()));
    assert_eq!(pipeline.downgrades(), Some([].as_slice()));
}

#[test]
fn with_no_required_path_every_bound_node_contributes() {
    let known = Known::new(vec![
        adapter(vec![OutputStreaming, ToolCalling, Interruptible]),
        adapter(vec![Interruptible, OutputStreaming]),
    ]);
    let mut declaration = Declaration::counted();
    let (pipeline_id, ()) = declaration
        .build(
            &known,
            known.profile.profile_id(),
            spec(Vec::new()),
            |pipeline| {
                let first = pipeline.node("stt", NodeRole::Model, Vec::new());
                let unbound = pipeline.node("transform", NodeRole::Transform, Vec::new());
                let second = pipeline.node("llm", NodeRole::Model, Vec::new());
                chain(
                    pipeline,
                    &[&first, &unbound, &second],
                    PortTraits::default(),
                );
                pipeline
                    .bind(&first, known.adapters[0].adapter_id())
                    .expect("the adapter is declared");
                pipeline
                    .bind(&second, known.adapters[1].adapter_id())
                    .expect("the adapter is declared");
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("the graph is valid");
    assert_eq!(
        declaration
            .plan(validated, &known)
            .expect("planned")
            .effective_capabilities(),
        [OutputStreaming, Interruptible]
    );

    let mut declaration = Declaration::counted();
    let (pipeline_id, ()) = declaration
        .build(
            &known,
            known.profile.profile_id(),
            spec(vec![ToolCalling]),
            |pipeline| {
                let first = pipeline.node("stt", NodeRole::Model, Vec::new());
                let second = pipeline.node("llm", NodeRole::Model, Vec::new());
                pipeline
                    .bind(&first, known.adapters[0].adapter_id())
                    .expect("the adapter is declared");
                pipeline
                    .bind(&second, known.adapters[1].adapter_id())
                    .expect("the adapter is declared");
            },
        )
        .expect("the profile is declared");
    assert_eq!(
        declaration.plan_of(pipeline_id, &known),
        Err(PlanPipelineError::Mismatch(CapabilityMismatch {
            node_id: None,
            capability: ToolCalling,
        })),
        "one bound adapter lacks what the pipeline requires"
    );
}

#[test]
fn an_external_provider_under_offline_strict_is_refused_naming_node_adapter_locality_and_profile() {
    let known = Known::new(vec![adapter_at(
        Locality::ExternalProvider,
        vec![ToolCalling],
    )]);
    let (mut declaration, pipeline_id, model) =
        source_model_sink(&known, Vec::new(), vec![ToolCalling], Some(0));
    let validated = declaration
        .validate(pipeline_id)
        .expect("the graph is valid");

    let denied = LocalityDenied {
        node_id: model,
        adapter_id: known.adapters[0].adapter_id(),
        locality: Locality::ExternalProvider,
        profile_id: known.profile.profile_id(),
    };
    assert_eq!(
        declaration.plan(validated, &known),
        Err(PlanPipelineError::LocalityDenied(denied))
    );
    assert_eq!(
        declaration.plan_pipeline(PlanPipeline { pipeline_id }, &known),
        Err(PlanPipelineError::LocalityDenied(denied)),
        "the command by identity refuses the same"
    );
    let pipeline = declaration.pipeline(pipeline_id).expect("declared");
    assert_eq!(pipeline.state(), PipelineState::Validated, "nothing moved");
    assert_eq!(pipeline.effective_capabilities(), None);
    assert_eq!(pipeline.downgrades(), None);
}

#[test]
fn the_admitted_list_is_read_as_declared() {
    for (kind, admitted, locality, plans) in [
        (
            ProfileKind::OfflineStrict,
            vec![Locality::SameProcess],
            Locality::LocalWorker,
            false,
        ),
        (
            ProfileKind::OfflineStrict,
            vec![Locality::SameProcess, Locality::LocalWorker],
            Locality::LocalWorker,
            true,
        ),
        (
            ProfileKind::OfflineStrict,
            Vec::new(),
            Locality::SameProcess,
            false,
        ),
        (
            ProfileKind::HybridOptIn,
            vec![Locality::ExternalProvider],
            Locality::ExternalProvider,
            true,
        ),
        (
            ProfileKind::HybridOptIn,
            vec![Locality::ExternalProvider],
            Locality::SameProcess,
            false,
        ),
    ] {
        let known = Known::under(kind, admitted.clone(), vec![adapter_at(locality, vec![])]);
        let (declaration, pipeline_id, model) =
            source_model_sink(&known, Vec::new(), Vec::new(), Some(0));
        let decided = declaration.plan_of(pipeline_id, &known);
        if plans {
            assert!(
                decided.is_ok(),
                "{kind:?} admitting {admitted:?}: {decided:?}"
            );
        } else {
            assert_eq!(
                decided,
                Err(PlanPipelineError::LocalityDenied(LocalityDenied {
                    node_id: model,
                    adapter_id: known.adapters[0].adapter_id(),
                    locality,
                    profile_id: known.profile.profile_id(),
                })),
                "{kind:?} admitting {admitted:?}"
            );
        }
    }
}

#[test]
fn a_denied_locality_is_refused_before_an_unbound_node_and_a_mismatch() {
    let known = Known::new(vec![
        adapter(vec![OutputStreaming]),
        adapter_at(Locality::ExternalProvider, vec![OutputStreaming]),
    ]);
    let mut declaration = Declaration::counted();
    let (pipeline_id, remote) = declaration
        .build(
            &known,
            known.profile.profile_id(),
            spec(Vec::new()),
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                let short = pipeline.node("short", NodeRole::Model, vec![ToolCalling]);
                let unbound = pipeline.node("unbound", NodeRole::Model, vec![RevisableInput]);
                let remote = pipeline.node("remote", NodeRole::Model, Vec::new());
                let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
                chain(
                    pipeline,
                    &[&source, &short, &unbound, &remote, &sink],
                    PortTraits::default(),
                );
                pipeline
                    .bind(&short, known.adapters[0].adapter_id())
                    .expect("the adapter is declared");
                pipeline
                    .bind(&remote, known.adapters[1].adapter_id())
                    .expect("the adapter is declared");
                remote.id()
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("the graph is valid");
    assert_eq!(
        declaration.plan(validated, &known),
        Err(PlanPipelineError::LocalityDenied(LocalityDenied {
            node_id: remote,
            adapter_id: known.adapters[1].adapter_id(),
            locality: Locality::ExternalProvider,
            profile_id: known.profile.profile_id(),
        }))
    );
}

#[test]
fn a_profile_the_catalog_does_not_hold_admits_nothing() {
    let known = Known::new(vec![adapter(Vec::new())]);
    let (declaration, pipeline_id, model) =
        source_model_sink(&known, Vec::new(), Vec::new(), Some(0));
    let elsewhere = Known {
        profile: DeploymentProfile::declare(DeclareProfile {
            kind: ProfileKind::OfflineStrict,
            egress: Egress::Denied,
            admitted: vec![Locality::SameProcess],
        })
        .expect("a row of the table")
        .0,
        adapters: known.adapters.clone(),
    };
    assert_eq!(
        declaration.plan_of(pipeline_id, &elsewhere),
        Err(PlanPipelineError::LocalityDenied(LocalityDenied {
            node_id: model,
            adapter_id: known.adapters[0].adapter_id(),
            locality: Locality::SameProcess,
            profile_id: known.profile.profile_id(),
        }))
    );
}

#[test]
fn only_a_validated_pipeline_is_planned_and_only_once() {
    let known = Known::new(Vec::new());
    let mut declaration = Declaration::counted();
    let (pipeline_id, ()) = declaration
        .build(&known, known.profile.profile_id(), spec(Vec::new()), |_| ())
        .expect("the profile is declared");
    let plan = PlanPipeline { pipeline_id };

    assert_eq!(
        declaration.plan_pipeline(plan, &known),
        Err(PlanPipelineError::WrongState(PipelineStateConflict {
            current: PipelineState::Declared
        }))
    );
    assert_eq!(state(&declaration, pipeline_id), PipelineState::Declared);
    let validated = declaration.validate(pipeline_id).expect("valid");
    assert_eq!(validated.pipeline_id(), pipeline_id);
    let planned = declaration.plan(validated, &known).expect("planned");
    assert_eq!(planned.pipeline_id(), pipeline_id);
    let planned_again = declaration.plan_pipeline(plan, &known);
    assert_eq!(
        planned_again,
        Err(PlanPipelineError::WrongState(PipelineStateConflict {
            current: PipelineState::Planned
        }))
    );
    assert_eq!(
        declaration.validate(pipeline_id),
        Err(ValidatePipelineError::WrongState(PipelineStateConflict {
            current: PipelineState::Planned
        }))
    );
    let unknown = PipelineId::from(Uuid::from_u128(0xdead));
    assert_eq!(
        declaration.plan_pipeline(
            PlanPipeline {
                pipeline_id: unknown
            },
            &known
        ),
        Err(PlanPipelineError::UnknownPipeline(PipelineNotFound {
            pipeline_id: unknown
        }))
    );
}

/// A random acyclic graph: node roles, edges from a lower to a higher index with the
/// essentiality of their two ports, each node's binding to one of four adapters, the adapters'
/// capabilities, and which adapters the catalog holds.
#[derive(Debug, Clone)]
struct Arrangement {
    roles: Vec<NodeRole>,
    edges: Vec<(usize, usize, bool, bool)>,
    bindings: Vec<Option<usize>>,
    adapters: Vec<Vec<Capability>>,
    held: Vec<bool>,
}

const ADAPTERS: usize = 4;

fn arrangement() -> impl Strategy<Value = Arrangement> {
    (2_usize..=7).prop_flat_map(|nodes| {
        let pairs: Vec<(usize, usize)> = (0..nodes)
            .flat_map(|from| (from + 1..nodes).map(move |to| (from, to)))
            .collect();
        (
            proptest::collection::vec(proptest::sample::select(NodeRole::ALL.to_vec()), nodes),
            proptest::collection::vec(
                proptest::option::weighted(0.45, (any::<bool>(), any::<bool>())),
                pairs.len(),
            ),
            proptest::collection::vec(proptest::option::of(0..ADAPTERS), nodes),
            proptest::collection::vec(
                proptest::sample::subsequence(Capability::ALL.to_vec(), 0..=Capability::ALL.len()),
                ADAPTERS,
            ),
            proptest::collection::vec(proptest::bool::weighted(0.85), ADAPTERS),
        )
            .prop_map(move |(mut roles, edges, bindings, adapters, held)| {
                let edges: Vec<_> = pairs
                    .iter()
                    .zip(edges)
                    .filter_map(|(&(from, to), essential)| {
                        essential.map(|(from_essential, to_essential)| {
                            (from, to, from_essential, to_essential)
                        })
                    })
                    .collect();
                // Check 6 refuses a sink no source reaches, so such a node is a transform.
                let mut reached = vec![false; roles.len()];
                for (index, role) in roles.iter().enumerate() {
                    reached[index] = *role == NodeRole::Source;
                }
                for &(from, to, _, _) in &edges {
                    if reached[from] {
                        reached[to] = true;
                    }
                }
                for (role, reached) in roles.iter_mut().zip(&reached) {
                    if *role == NodeRole::Sink && !reached {
                        *role = NodeRole::Transform;
                    }
                }
                Arrangement {
                    roles,
                    edges,
                    bindings,
                    adapters,
                    held,
                }
            })
    })
}

/// Every node on some path from a source to a sink along edges whose two ports are essential,
/// found by enumerating the paths one by one.
fn on_required_paths(arrangement: &Arrangement) -> BTreeSet<usize> {
    fn walk(arrangement: &Arrangement, path: &mut Vec<usize>, found: &mut BTreeSet<usize>) {
        let last = *path.last().expect("a path has a start");
        if arrangement.roles[last] == NodeRole::Sink {
            found.extend(path.iter().copied());
        }
        for &(from, to, from_essential, to_essential) in &arrangement.edges {
            if from == last && from_essential && to_essential {
                path.push(to);
                walk(arrangement, path, found);
                path.pop();
            }
        }
    }
    let mut found = BTreeSet::new();
    for (start, role) in arrangement.roles.iter().enumerate() {
        if *role == NodeRole::Source {
            walk(arrangement, &mut vec![start], &mut found);
        }
    }
    found
}

/// The intersection over the bound nodes on every required path, in declaration order.
/// With no required path, every node contributes its binding.
fn expected(arrangement: &Arrangement) -> Vec<Capability> {
    let mut effective: Vec<Capability> = Capability::ALL.to_vec();
    let mut contributing = on_required_paths(arrangement);
    if contributing.is_empty() {
        contributing = (0..arrangement.roles.len()).collect();
    }
    for node in contributing {
        if let Some(index) = arrangement.bindings[node] {
            let implemented: &[Capability] = if arrangement.held[index] {
                &arrangement.adapters[index]
            } else {
                &[]
            };
            effective.retain(|capability| implemented.contains(capability));
        }
    }
    effective
}

proptest! {
    #[test]
    fn effective_capabilities_are_the_intersection_over_bound_nodes_on_every_required_path(
        arrangement in arrangement()
    ) {
        let adapters: Vec<Adapter> = arrangement.adapters.iter().cloned().map(adapter).collect();
        let known = Known::new(
            adapters
                .iter()
                .zip(&arrangement.held)
                .filter(|(_, held)| **held)
                .map(|(adapter, _)| adapter.clone())
                .collect(),
        );
        let binding_time = Known {
            profile: known.profile.clone(),
            adapters: adapters.clone(),
        };
        let mut declaration = Declaration::counted();
        let (pipeline_id, ()) = declaration.build(&binding_time, known.profile.profile_id(), spec(Vec::new()), |pipeline| {
            let nodes: Vec<_> = arrangement
                .roles
                .iter()
                .map(|role| pipeline.node(format!("{role:?}"), *role, Vec::new()))
                .collect();
            for &(from, to, from_essential, to_essential) in &arrangement.edges {
                let traits = |essential: bool| if essential { PortTraits::default() } else { optional() };
                let out = pipeline.output::<modality::Text>(&nodes[from], "out", traits(from_essential));
                let into = pipeline.input::<modality::Text>(&nodes[to], "in", traits(to_essential));
                pipeline.connect(&out, &into, NonZeroUsize::MIN, OverloadPolicy::Pause);
            }
            for (node, binding) in nodes.iter().zip(&arrangement.bindings) {
                if let Some(index) = binding {
                    pipeline.bind(node, adapters[*index].adapter_id())
.expect("the adapter is declared");
                }
            }
        })
.expect("the profile is declared");
        let validated = declaration.validate(pipeline_id);
        prop_assert!(validated.is_ok(), "{validated:?}");
        let planned = declaration
            .plan(validated.expect("checked"), &known)
            .expect("no node requires anything");

        let expected = expected(&arrangement);
        prop_assert_eq!(planned.effective_capabilities(), expected.as_slice());
        prop_assert_eq!(
            declaration.pipeline(pipeline_id).and_then(|pipeline| pipeline.effective_capabilities()),
            Some(expected.as_slice())
        );
    }
}
