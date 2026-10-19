//! The declaration, validation and planning half of the `inference.graph` domain of
//! `ess/domains/graph.yaml`, held in memory.
//!
//! [`GraphDomain`] holds one [`Declaration`] of `inference-pipeline` with counted identities, so
//! that two runs of the same commands report the same identities, and records every event its
//! commands emit in order. The commands are the declaration's own, `ValidatePipeline` and
//! `PlanPipeline` among them; what a declaration references and does not hold, the declared
//! profiles and adapters, it reads through [`GraphCatalog`] over the [`PolicyDomain`] and
//! [`ModelDomain`] stores. A refused command stores nothing, records no event and takes no
//! identity. The views `inference.graph.Pipelines`, `Nodes`, `Ports`, `Edges` and `Bindings` are
//! the declaration's entities ([`GraphDomain::declaration`]), read at the call. Runs are not here
//! yet.

use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeploymentProfile, ProfileId, ProfileNotFound};
use inference_pipeline::{
    AddNode, AddNodeError, AddPort, BindNode, BindNodeError, Catalog, ConnectPorts,
    ConnectPortsError, Declaration, DeclarePipeline, NodeAdded, NodeBound, NodeNotFound,
    PipelineDeclared, PipelinePlanned, PipelineValidated, PlanPipeline, PlanPipelineError,
    PortAdded, PortsConnected, ValidatePipeline, ValidatePipelineError,
};

use crate::model::ModelDomain;
use crate::policy::PolicyDomain;

/// An event of the declaration, validation and planning half of `inference.graph`, as the domain
/// emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphEvent {
    /// `inference.graph.PipelineDeclared`.
    PipelineDeclared(PipelineDeclared),
    /// `inference.graph.NodeAdded`.
    NodeAdded(NodeAdded),
    /// `inference.graph.PortAdded`.
    PortAdded(PortAdded),
    /// `inference.graph.PortsConnected`.
    PortsConnected(PortsConnected),
    /// `inference.graph.NodeBound`.
    NodeBound(NodeBound),
    /// `inference.graph.PipelineValidated`.
    PipelineValidated(PipelineValidated),
    /// `inference.graph.PipelinePlanned`.
    PipelinePlanned(PipelinePlanned),
}

/// The declared profiles and adapters a [`GraphDomain`] reads, from their own stores.
#[derive(Debug, Clone, Copy)]
pub struct GraphCatalog<'a> {
    pub policy: &'a PolicyDomain,
    pub model: &'a ModelDomain,
}

impl Catalog for GraphCatalog<'_> {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        self.policy.profile(profile_id)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.model.adapters().get(adapter_id)
    }
}

/// The declaration half of `inference.graph` in memory: the declared entities and the events
/// their commands emitted.
#[derive(Debug, Clone)]
pub struct GraphDomain {
    declaration: Declaration,
    events: Vec<GraphEvent>,
}

impl Default for GraphDomain {
    fn default() -> Self {
        Self {
            declaration: Declaration::counted(),
            events: Vec::new(),
        }
    }
}

impl GraphDomain {
    /// An empty domain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `inference.graph.DeclarePipeline` and records its [`PipelineDeclared`].
    ///
    /// # Errors
    ///
    /// [`ProfileNotFound`] when `catalog` holds no profile carrying `profile_id`.
    pub fn declare_pipeline(
        &mut self,
        input: DeclarePipeline,
        catalog: &impl Catalog,
    ) -> Result<PipelineDeclared, ProfileNotFound> {
        let event = self.declaration.declare_pipeline(input, catalog)?;
        self.events
            .push(GraphEvent::PipelineDeclared(event.clone()));
        Ok(event)
    }

    /// Runs `inference.graph.AddNode` and records its [`NodeAdded`].
    ///
    /// # Errors
    ///
    /// As [`Declaration::add_node`].
    pub fn add_node(&mut self, input: AddNode) -> Result<NodeAdded, AddNodeError> {
        let event = self.declaration.add_node(input)?;
        self.events.push(GraphEvent::NodeAdded(event.clone()));
        Ok(event)
    }

    /// Runs `inference.graph.AddPort` and records its [`PortAdded`].
    ///
    /// # Errors
    ///
    /// As [`Declaration::add_port`].
    pub fn add_port(&mut self, input: AddPort) -> Result<PortAdded, NodeNotFound> {
        let event = self.declaration.add_port(input)?;
        self.events.push(GraphEvent::PortAdded(event.clone()));
        Ok(event)
    }

    /// Runs `inference.graph.ConnectPorts` and records its [`PortsConnected`].
    ///
    /// # Errors
    ///
    /// As [`Declaration::connect_ports`].
    pub fn connect_ports(
        &mut self,
        input: ConnectPorts,
    ) -> Result<PortsConnected, ConnectPortsError> {
        let event = self.declaration.connect_ports(input)?;
        self.events.push(GraphEvent::PortsConnected(event));
        Ok(event)
    }

    /// Runs `inference.graph.BindNode` and records its [`NodeBound`].
    ///
    /// # Errors
    ///
    /// As [`Declaration::bind_node`].
    pub fn bind_node(
        &mut self,
        input: BindNode,
        catalog: &impl Catalog,
    ) -> Result<NodeBound, BindNodeError> {
        let event = self.declaration.bind_node(input, catalog)?;
        self.events.push(GraphEvent::NodeBound(event));
        Ok(event)
    }

    /// Runs `inference.graph.ValidatePipeline` and records its [`PipelineValidated`].
    ///
    /// # Errors
    ///
    /// As [`Declaration::validate_pipeline`].
    pub fn validate_pipeline(
        &mut self,
        input: ValidatePipeline,
    ) -> Result<PipelineValidated, ValidatePipelineError> {
        let event = self.declaration.validate_pipeline(input)?;
        self.events.push(GraphEvent::PipelineValidated(event));
        Ok(event)
    }

    /// Runs `inference.graph.PlanPipeline` and records its [`PipelinePlanned`].
    ///
    /// # Errors
    ///
    /// As [`Declaration::plan_pipeline`].
    pub fn plan_pipeline(
        &mut self,
        input: PlanPipeline,
        catalog: &impl Catalog,
    ) -> Result<PipelinePlanned, PlanPipelineError> {
        let event = self.declaration.plan_pipeline(input, catalog)?;
        self.events.push(GraphEvent::PipelinePlanned(event));
        Ok(event)
    }

    /// The declared pipelines and what they own.
    #[must_use]
    pub const fn declaration(&self) -> &Declaration {
        &self.declaration
    }

    /// Every event emitted, in the order it was emitted.
    #[must_use]
    pub fn events(&self) -> &[GraphEvent] {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use inference_core::model::{
        Capability, DeclareAdapter, DeclareModel, ExecutionForm, Locality, Modality,
    };
    use inference_core::policy::{DeclareProfile, Egress, ProfileKind, ProfileNotFound};
    use inference_pipeline::{
        AddNode, AddNodeError, DeclarePipeline, InteractionMode, NodeRole, PipelineId,
        PipelineNotFound,
    };
    use uuid::Uuid;

    use super::{GraphCatalog, GraphDomain, GraphEvent};
    use crate::model::ModelDomain;
    use crate::policy::PolicyDomain;

    fn stores() -> (PolicyDomain, ModelDomain) {
        let mut policy = PolicyDomain::new();
        policy
            .declare_profile(DeclareProfile {
                kind: ProfileKind::OfflineStrict,
                egress: Egress::Denied,
                admitted: vec![Locality::SameProcess],
            })
            .expect("a row of the table");
        let mut model = ModelDomain::new();
        let declared = model.declare_model(DeclareModel {
            family: "f".to_owned(),
            checkpoint: "c".to_owned(),
            code_license: "l".to_owned(),
            weight_license: "w".to_owned(),
            checksum: "s".to_owned(),
            input_modalities: vec![Modality::Text],
            output_modalities: vec![Modality::Text],
            provenance: "p".to_owned(),
            source_revision: "r".to_owned(),
        });
        model
            .declare_adapter(DeclareAdapter {
                model_id: declared.model_id,
                backend: "b".to_owned(),
                contract_revision: "c".to_owned(),
                execution_form: ExecutionForm::InProcess,
                locality: Locality::SameProcess,
                capabilities: vec![Capability::OutputStreaming],
                max_concurrency: 1,
            })
            .expect("declared");
        (policy, model)
    }

    fn declare_pipeline(policy: &PolicyDomain) -> DeclarePipeline {
        DeclarePipeline {
            name: "p".to_owned(),
            schema_revision: "1".to_owned(),
            mode: InteractionMode::Batch,
            profile_id: policy.profiles().next().expect("a profile").profile_id(),
            required_capabilities: Vec::new(),
        }
    }

    #[test]
    fn identities_come_from_a_counter_and_a_refusal_records_and_takes_nothing() {
        let (policy, model) = stores();
        let catalog = GraphCatalog {
            policy: &policy,
            model: &model,
        };
        let mut domain = GraphDomain::new();
        let profile_id = Uuid::from_u128(0xdead).into();

        assert_eq!(
            domain.declare_pipeline(
                DeclarePipeline {
                    profile_id,
                    ..declare_pipeline(&policy)
                },
                &catalog
            ),
            Err(ProfileNotFound { profile_id })
        );
        let unknown = PipelineId::from(Uuid::from_u128(0xbeef));
        assert_eq!(
            domain.add_node(AddNode {
                pipeline_id: unknown,
                name: "n".to_owned(),
                role: NodeRole::Source,
                requires: Vec::new(),
            }),
            Err(AddNodeError::UnknownPipeline(PipelineNotFound {
                pipeline_id: unknown
            }))
        );
        assert!(domain.events().is_empty());

        let declared = domain
            .declare_pipeline(declare_pipeline(&policy), &catalog)
            .expect("the profile is declared");
        let added = domain
            .add_node(AddNode {
                pipeline_id: declared.pipeline_id,
                name: "n".to_owned(),
                role: NodeRole::Source,
                requires: Vec::new(),
            })
            .expect("the pipeline is declared");
        assert_eq!(
            declared.pipeline_id.to_string(),
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(
            added.node_id.to_string(),
            "00000000-0000-4000-8000-000000000002"
        );
        assert_eq!(
            domain.events(),
            [
                GraphEvent::PipelineDeclared(declared),
                GraphEvent::NodeAdded(added)
            ]
        );
    }

    #[test]
    fn a_validation_records_its_event_and_a_refused_one_records_nothing() {
        use inference_pipeline::{
            PipelineState, PipelineStateConflict, PipelineValidated, ValidatePipeline,
            ValidatePipelineError,
        };

        let (policy, model) = stores();
        let catalog = GraphCatalog {
            policy: &policy,
            model: &model,
        };
        let mut domain = GraphDomain::new();
        let pipeline_id = domain
            .declare_pipeline(declare_pipeline(&policy), &catalog)
            .expect("the profile is declared")
            .pipeline_id;
        let validate = ValidatePipeline { pipeline_id };

        assert_eq!(
            domain.validate_pipeline(validate),
            Ok(PipelineValidated { pipeline_id })
        );
        assert_eq!(
            domain.validate_pipeline(validate),
            Err(ValidatePipelineError::WrongState(PipelineStateConflict {
                current: PipelineState::Validated
            }))
        );
        assert_eq!(domain.events().len(), 2);
        assert_eq!(
            domain.events()[1],
            GraphEvent::PipelineValidated(PipelineValidated { pipeline_id })
        );
    }

    #[test]
    fn a_plan_records_its_event_and_a_refused_one_records_nothing() {
        use inference_pipeline::{
            CapabilityMismatch, PipelinePlanned, PipelineState, PipelineStateConflict,
            PlanPipeline, PlanPipelineError, ValidatePipeline,
        };

        let (policy, model) = stores();
        let catalog = GraphCatalog {
            policy: &policy,
            model: &model,
        };
        let mut domain = GraphDomain::new();
        let pipeline_id = domain
            .declare_pipeline(
                DeclarePipeline {
                    required_capabilities: vec![Capability::OutputStreaming],
                    ..declare_pipeline(&policy)
                },
                &catalog,
            )
            .expect("the profile is declared")
            .pipeline_id;
        let plan = PlanPipeline { pipeline_id };

        assert_eq!(
            domain.plan_pipeline(plan, &catalog),
            Err(PlanPipelineError::WrongState(PipelineStateConflict {
                current: PipelineState::Declared
            }))
        );
        domain
            .validate_pipeline(ValidatePipeline { pipeline_id })
            .expect("an empty graph is valid");
        assert_eq!(
            domain.plan_pipeline(plan, &catalog),
            Ok(PipelinePlanned { pipeline_id })
        );
        assert_eq!(
            domain.events().last(),
            Some(&GraphEvent::PipelinePlanned(PipelinePlanned {
                pipeline_id
            }))
        );
        assert_eq!(domain.events().len(), 3);
        let pipeline = domain
            .declaration()
            .pipeline(pipeline_id)
            .expect("declared");
        assert_eq!(pipeline.state(), PipelineState::Planned);
        assert_eq!(
            pipeline.effective_capabilities(),
            Some([].as_slice()),
            "a pipeline with no node has no effective capability"
        );

        let short = domain
            .declare_pipeline(
                DeclarePipeline {
                    required_capabilities: vec![Capability::ToolCalling],
                    ..declare_pipeline(&policy)
                },
                &catalog,
            )
            .expect("the profile is declared")
            .pipeline_id;
        let source = domain
            .add_node(AddNode {
                pipeline_id: short,
                name: "llm".to_owned(),
                role: NodeRole::Model,
                requires: vec![Capability::ToolCalling],
            })
            .expect("the pipeline is declared")
            .node_id;
        let adapter_id = model
            .adapters()
            .iter()
            .next()
            .expect("an adapter")
            .adapter_id();
        domain
            .bind_node(
                inference_pipeline::BindNode {
                    pipeline_id: short,
                    node_id: source,
                    adapter_id,
                },
                &catalog,
            )
            .expect("the adapter is declared");
        domain
            .validate_pipeline(ValidatePipeline { pipeline_id: short })
            .expect("a lone node with no port is valid");
        let recorded = domain.events().len();
        assert_eq!(
            domain.plan_pipeline(PlanPipeline { pipeline_id: short }, &catalog),
            Err(PlanPipelineError::Mismatch(CapabilityMismatch {
                node_id: Some(source),
                capability: Capability::ToolCalling,
            }))
        );
        assert_eq!(domain.events().len(), recorded);
    }

    #[test]
    fn the_catalog_reads_the_profile_and_adapter_stores() {
        use inference_pipeline::Catalog;

        let (policy, model) = stores();
        let catalog = GraphCatalog {
            policy: &policy,
            model: &model,
        };
        let profile = policy.profiles().next().expect("a profile").profile_id();
        let adapter = model
            .adapters()
            .iter()
            .next()
            .expect("an adapter")
            .adapter_id();
        assert!(catalog.profile(profile).is_some());
        assert!(catalog.adapter(adapter).is_some());
        assert!(catalog.profile(Uuid::from_u128(0xdead).into()).is_none());
        assert!(catalog.adapter(Uuid::from_u128(0xdead).into()).is_none());
    }
}
