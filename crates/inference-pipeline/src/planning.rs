//! `inference.graph.PlanPipeline`: check 4 of §10.2 and the capability rule of §8.3 over one
//! validated pipeline.
//!
//! Effective capabilities are computed, never a union of adapter feature lists: the weakest
//! required node decides. A required path runs from a node of role `source` to a node of role
//! `sink` along edges whose two ports are both `essential`. A node on a required path that has a
//! binding contributes the capabilities of the adapter its binding references, and a node without
//! one contributes nothing and is transparent; a binding whose adapter the catalog does not hold
//! contributes an adapter that implements nothing. When the pipeline has no required path, every
//! node contributes as if it were on one. A node bound more than once contributes each binding.
//! The effective capabilities are the intersection of the contributions, in the declaration order
//! of [`Capability`]; over no contribution they are every capability, except that a pipeline with
//! no node has none.
//!
//! A subgraph node (one whose [`Node::subgraph_id`] names a Planned pipeline) is bound to nothing:
//! it contributes its child's effective capabilities as a bound node contributes its adapter's.
//! Before anything else, planning refuses a binding whose adapter runs at a locality the
//! pipeline's deployment profile does not admit (A-04 of the design, line 1140: the offline
//! profile refuses a hosted adapter): [`LocalityDenied`] naming the node, the adapter, its
//! locality and the profile, the first such binding with nodes in declaration order. The check
//! covers every binding reachable through subgraph nodes, against the pipeline's own profile: a
//! subgraph node's child, planned under a profile of its own, runs inside the pipeline, and a
//! binding inside it is refused naming the inner node, at the subgraph node's place in the order.
//! The
//! `admitted` list is read as declared; the table rules of `inference.policy.DeclareProfile`
//! already decide which localities a kind may admit, and planning adds none. A binding whose
//! adapter the catalog does not hold has no locality and is not refused for one; a profile the
//! catalog does not hold admits nothing. Next, planning refuses a subgraph node whose ports and
//! its child's exposures do not match one to one by name, direction and modality
//! ([`SubgraphInterfaceMismatch`], the first such node in declaration order), with one exception,
//! the usage roll-up: the node's structured output port `usage` also matches every egress of the
//! child that exposes a structured output port `usage` and whose name no port of the node carries
//! ([`Declaration::forwarded_usage`]); such a port matching no egress at all is refused.
//!
//! Planning refuses, in this order: a node with a non-empty `requires` and no binding
//! ([`NodeUnbound`], the first such node in declaration order); a node requiring a capability a
//! bound adapter of it does not implement ([`CapabilityMismatch`] naming the node and its first
//! unmet capability, nodes in declaration order); a capability the pipeline requires that a
//! contributing adapter does not implement ([`CapabilityMismatch`] naming no node). A pipeline
//! no adapter contributes to is refused for no requirement of its own: no bound adapter falls
//! short of it, so a pipeline with no node plans with no effective capability whatever it
//! requires. Downgrades are always empty: no preference list exists to downgrade from, so nothing
//! optional is ever dropped.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use inference_core::model::{Adapter, Capability, Locality};
use inference_core::policy::{DeploymentProfile, ProfileId};

use crate::declaration::carries_usage;
use crate::{
    CapabilityMismatch, Catalog, Declaration, Essentiality, LocalityDenied, Node, NodeId, NodeRole,
    NodeUnbound, Pipeline, PipelineId, PipelineNotFound, PipelinePlanned, PipelineState,
    PipelineStateConflict, PlanPipeline, PortId, SubgraphInterfaceMismatch, ValidatePipeline,
    ValidatePipelineError,
};

/// The validated form of a pipeline: only [`Declaration::validate`] produces one, from the
/// `validated` outcome of `inference.graph.ValidatePipeline`, and [`Declaration::plan`] takes it.
#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct ValidatedPipeline {
    pipeline_id: PipelineId,
}

impl ValidatedPipeline {
    /// The validated pipeline's identity.
    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }
}

/// The planned form of a pipeline: only planning produces one, and it carries what planning
/// computed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub struct PlannedPipeline {
    pipeline_id: PipelineId,
    effective_capabilities: Vec<Capability>,
    downgrades: Vec<Capability>,
}

impl PlannedPipeline {
    /// The planned pipeline's identity.
    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    /// The capabilities every required path implements, in the declaration order of
    /// [`Capability`].
    #[must_use]
    pub fn effective_capabilities(&self) -> &[Capability] {
        &self.effective_capabilities
    }

    /// The required or preferred capabilities planning could not honour; empty, as no preference
    /// list exists yet.
    #[must_use]
    pub fn downgrades(&self) -> &[Capability] {
        &self.downgrades
    }
}

/// Why `inference.graph.PlanPipeline` moved nothing. The refusals are decided in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanPipelineError {
    /// No pipeline carries the identity, so none is in the state the command acts from; the
    /// `wrong-state` outcome, with no state to report.
    UnknownPipeline(PipelineNotFound),
    /// The `wrong-state` outcome: the pipeline is not Validated.
    WrongState(PipelineStateConflict),
    /// The `interface-mismatch` outcome: a subgraph node's ports do not match its child's
    /// exposures.
    InterfaceMismatch(SubgraphInterfaceMismatch),
    /// The `unbound` outcome: a node whose requirements need an adapter has no binding.
    Unbound(NodeUnbound),
    /// The `mismatch` outcome: a mandatory capability of a node or of the pipeline is not
    /// implemented.
    Mismatch(CapabilityMismatch),
    /// The `locality-denied` outcome: a bound adapter runs at a locality the pipeline's
    /// deployment profile does not admit.
    LocalityDenied(LocalityDenied),
}

impl fmt::Display for PlanPipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
            Self::WrongState(error) => fmt::Display::fmt(error, f),
            Self::InterfaceMismatch(error) => fmt::Display::fmt(error, f),
            Self::Unbound(error) => fmt::Display::fmt(error, f),
            Self::Mismatch(error) => fmt::Display::fmt(error, f),
            Self::LocalityDenied(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for PlanPipelineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownPipeline(error) => Some(error),
            Self::WrongState(error) => Some(error),
            Self::InterfaceMismatch(error) => Some(error),
            Self::Unbound(error) => Some(error),
            Self::Mismatch(error) => Some(error),
            Self::LocalityDenied(error) => Some(error),
        }
    }
}

impl Declaration {
    /// Runs `inference.graph.ValidatePipeline` as [`Declaration::validate_pipeline`] does, and
    /// answers its `validated` outcome as the validated form that planning takes.
    ///
    /// # Errors
    ///
    /// As [`Declaration::validate_pipeline`].
    pub fn validate(
        &mut self,
        pipeline_id: PipelineId,
    ) -> Result<ValidatedPipeline, ValidatePipelineError> {
        let validated = self.validate_pipeline(ValidatePipeline { pipeline_id })?;
        Ok(ValidatedPipeline {
            pipeline_id: validated.pipeline_id,
        })
    }

    /// Plans a validated pipeline: `inference.graph.PlanPipeline` over the pipeline `pipeline`
    /// names, answering the planned form:
    ///
    /// ```
    /// # use inference_core::policy::ProfileId;
    /// # use inference_pipeline::{Catalog, Declaration, InteractionMode, PipelineSpec};
    /// # fn plan(catalog: &impl Catalog, profile_id: ProfileId) -> Result<(), Box<dyn std::error::Error>> {
    /// # let spec = PipelineSpec { name: "n".to_owned(), schema_revision: "1".to_owned(), mode: InteractionMode::Batch, required_capabilities: Vec::new() };
    /// let mut declaration = Declaration::new();
    /// let (pipeline_id, ()) = declaration.build(catalog, profile_id, spec, |_| ())?;
    /// let validated = declaration.validate(pipeline_id)?;
    /// declaration.plan(validated, catalog)?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// A pipeline that was not validated has no [`ValidatedPipeline`], so it cannot be planned
    /// here:
    ///
    /// ```compile_fail
    /// # use inference_core::policy::ProfileId;
    /// # use inference_pipeline::{Catalog, Declaration, InteractionMode, PipelineSpec};
    /// # fn plan(catalog: &impl Catalog, profile_id: ProfileId) -> Result<(), Box<dyn std::error::Error>> {
    /// # let spec = PipelineSpec { name: "n".to_owned(), schema_revision: "1".to_owned(), mode: InteractionMode::Batch, required_capabilities: Vec::new() };
    /// let mut declaration = Declaration::new();
    /// let (pipeline_id, ()) = declaration.build(catalog, profile_id, spec, |_| ())?;
    /// declaration.plan(pipeline_id, catalog)?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// As [`Declaration::plan_pipeline`].
    pub fn plan(
        &mut self,
        pipeline: ValidatedPipeline,
        catalog: &(impl Catalog + ?Sized),
    ) -> Result<PlannedPipeline, PlanPipelineError> {
        let pipeline_id = pipeline.pipeline_id;
        let effective_capabilities = self.planned(pipeline_id, catalog)?;
        Ok(PlannedPipeline {
            pipeline_id,
            effective_capabilities,
            downgrades: Vec::new(),
        })
    }

    /// Runs `inference.graph.PlanPipeline`: computes the pipeline's effective capabilities
    /// ([`Declaration::plan_of`]), records them with empty downgrades, and moves the pipeline from
    /// Validated to Planned.
    ///
    /// # Errors
    ///
    /// In this order: [`PlanPipelineError::UnknownPipeline`]; [`PlanPipelineError::WrongState`]
    /// for a pipeline that is not Validated; [`PlanPipelineError::LocalityDenied`],
    /// [`PlanPipelineError::InterfaceMismatch`], [`PlanPipelineError::Unbound`] and
    /// [`PlanPipelineError::Mismatch`] as [`Declaration::plan_of`] decides them. A refused plan
    /// moves nothing.
    pub fn plan_pipeline(
        &mut self,
        input: PlanPipeline,
        catalog: &(impl Catalog + ?Sized),
    ) -> Result<PipelinePlanned, PlanPipelineError> {
        let pipeline_id = input.pipeline_id;
        self.planned(pipeline_id, catalog)?;
        Ok(PipelinePlanned { pipeline_id })
    }

    /// What planning decides over the declaration of the pipeline carrying `pipeline_id`, in any
    /// state and moving nothing: its effective capabilities, or the refusal; see the module
    /// documentation of the rule.
    ///
    /// # Errors
    ///
    /// [`PlanPipelineError::UnknownPipeline`]; then [`PlanPipelineError::LocalityDenied`],
    /// [`PlanPipelineError::InterfaceMismatch`], [`PlanPipelineError::Unbound`] and
    /// [`PlanPipelineError::Mismatch`], never [`PlanPipelineError::WrongState`].
    pub fn plan_of(
        &self,
        pipeline_id: PipelineId,
        catalog: &(impl Catalog + ?Sized),
    ) -> Result<Vec<Capability>, PlanPipelineError> {
        let pipeline = self
            .pipeline(pipeline_id)
            .ok_or(PlanPipelineError::UnknownPipeline(PipelineNotFound {
                pipeline_id,
            }))?;
        let nodes: Vec<&Node> = self
            .nodes()
            .filter(|node| node.pipeline_id() == pipeline_id)
            .collect();
        let adapters = |node_id: NodeId| {
            self.bindings()
                .filter(move |binding| {
                    binding.pipeline_id() == pipeline_id && binding.node_id() == node_id
                })
                .map(|binding| {
                    catalog
                        .adapter(binding.adapter_id())
                        .map_or(&[][..], Adapter::capabilities)
                })
        };
        if let Some(denied) = self.locality_denied(pipeline, &nodes, catalog) {
            return Err(PlanPipelineError::LocalityDenied(denied));
        }
        if let Some(mismatch) = self.interface_mismatch(&nodes) {
            return Err(PlanPipelineError::InterfaceMismatch(mismatch));
        }
        let subgraph = |node_id: NodeId| {
            nodes
                .iter()
                .find(|node| node.node_id() == node_id)
                .and_then(|node| node.subgraph_id())
                .map(|child| {
                    self.pipeline(child)
                        .and_then(crate::Pipeline::effective_capabilities)
                        .unwrap_or(&[])
                })
        };
        if let Some(node) = nodes
            .iter()
            .find(|node| !node.requires().is_empty() && adapters(node.node_id()).next().is_none())
        {
            return Err(PlanPipelineError::Unbound(NodeUnbound {
                node_id: node.node_id(),
            }));
        }
        for node in &nodes {
            for implemented in adapters(node.node_id()) {
                if let Some(capability) = first_unmet(node.requires(), implemented) {
                    return Err(PlanPipelineError::Mismatch(CapabilityMismatch {
                        node_id: Some(node.node_id()),
                        capability,
                    }));
                }
            }
        }
        let mut contributing = self.on_required_paths(pipeline_id, &nodes);
        if contributing.is_empty() {
            contributing = nodes.iter().map(|node| node.node_id()).collect();
        }
        let mut effective = Capability::ALL.to_vec();
        for node_id in contributing {
            for implemented in adapters(node_id).chain(subgraph(node_id)) {
                effective.retain(|capability| implemented.contains(capability));
            }
        }
        if let Some(capability) = first_unmet(pipeline.required_capabilities(), &effective) {
            return Err(PlanPipelineError::Mismatch(CapabilityMismatch {
                node_id: None,
                capability,
            }));
        }
        if nodes.is_empty() {
            effective.clear();
        }
        Ok(effective)
    }

    /// Plans the pipeline carrying `pipeline_id` from Validated: records its effective
    /// capabilities and empty downgrades and moves it to Planned, or moves nothing.
    fn planned(
        &mut self,
        pipeline_id: PipelineId,
        catalog: &(impl Catalog + ?Sized),
    ) -> Result<Vec<Capability>, PlanPipelineError> {
        let current = self
            .pipeline(pipeline_id)
            .ok_or(PlanPipelineError::UnknownPipeline(PipelineNotFound {
                pipeline_id,
            }))?
            .state;
        if current != PipelineState::Validated {
            return Err(PlanPipelineError::WrongState(PipelineStateConflict {
                current,
            }));
        }
        let effective = self.plan_of(pipeline_id, catalog)?;
        if let Some(pipeline) = self.pipeline_mut(pipeline_id) {
            pipeline.effective_capabilities = Some(effective.clone());
            pipeline.downgrades = Some(Vec::new());
            pipeline.state = PipelineState::Planned;
        }
        Ok(effective)
    }

    /// The first binding reachable from `nodes` whose adapter the catalog holds and runs at a
    /// locality the `admitted` list of `pipeline`'s profile does not name, read as declared; a
    /// profile the catalog does not hold admits nothing. Nodes are read in declaration order, each
    /// node's own bindings in theirs, and a subgraph node then contributes every binding of its
    /// child pipeline, recursively, read the same way: a child planned under a profile of its own
    /// still runs inside the parent, so the parent's profile decides. The refusal names the node
    /// the binding belongs to, which for a binding inside a subgraph is the inner node.
    fn locality_denied(
        &self,
        pipeline: &Pipeline,
        nodes: &[&Node],
        catalog: &(impl Catalog + ?Sized),
    ) -> Option<LocalityDenied> {
        let profile_id = pipeline.profile_id();
        let admitted = catalog
            .profile(profile_id)
            .map_or(&[][..], DeploymentProfile::admitted);
        let mut visited = BTreeSet::from([pipeline.pipeline_id()]);
        self.locality_denied_within(nodes, admitted, profile_id, catalog, &mut visited)
    }

    /// [`Declaration::locality_denied`] over `nodes` against `admitted`, descending into the child
    /// of every subgraph node not yet in `visited`.
    fn locality_denied_within(
        &self,
        nodes: &[&Node],
        admitted: &[Locality],
        profile_id: ProfileId,
        catalog: &(impl Catalog + ?Sized),
        visited: &mut BTreeSet<PipelineId>,
    ) -> Option<LocalityDenied> {
        for node in nodes {
            let denied = self
                .bindings()
                .filter(|binding| {
                    binding.pipeline_id() == node.pipeline_id()
                        && binding.node_id() == node.node_id()
                })
                .find_map(|binding| {
                    let locality = catalog.adapter(binding.adapter_id())?.locality();
                    (!admitted.contains(&locality)).then_some(LocalityDenied {
                        node_id: node.node_id(),
                        adapter_id: binding.adapter_id(),
                        locality,
                        profile_id,
                    })
                });
            if denied.is_some() {
                return denied;
            }
            let Some(child) = node.subgraph_id() else {
                continue;
            };
            if !visited.insert(child) {
                continue;
            }
            let inner: Vec<&Node> = self
                .nodes()
                .filter(|inner| inner.pipeline_id() == child)
                .collect();
            if let Some(denied) =
                self.locality_denied_within(&inner, admitted, profile_id, catalog, visited)
            {
                return Some(denied);
            }
        }
        None
    }

    /// The first subgraph node of `nodes`, in declaration order, whose ports and its child's
    /// exposures do not match one to one by name, direction and modality, the usage roll-up
    /// aside, with what does not match: in the node's port order, a port no exposure carries the
    /// name of, unless it is the node's usage port and forwards an egress
    /// ([`Declaration::forwarded_usage`]), or one whose exposure of that name differs in direction
    /// or modality or was matched by an earlier port; then, in the child's exposure order, an
    /// exposure no port carries the name of and the usage port does not forward.
    fn interface_mismatch(&self, nodes: &[&Node]) -> Option<SubgraphInterfaceMismatch> {
        for node in nodes {
            let Some(child) = node.subgraph_id() else {
                continue;
            };
            let node_id = node.node_id();
            let ports: Vec<&crate::Port> = self
                .ports()
                .filter(|port| {
                    port.node_id() == node_id && port.pipeline_id() == node.pipeline_id()
                })
                .collect();
            let exposures: Vec<&crate::Exposure> = self
                .exposures()
                .filter(|exposure| exposure.pipeline_id() == child)
                .collect();
            let forwarded: BTreeSet<_> = self
                .forwarded_usage(node_id)
                .iter()
                .map(|exposure| exposure.exposure_id())
                .collect();
            let mut matched = forwarded.clone();
            for port in &ports {
                let mismatch = |exposure_id| SubgraphInterfaceMismatch {
                    node_id,
                    port_id: Some(port.port_id()),
                    exposure_id,
                };
                let Some(exposure) = exposures
                    .iter()
                    .find(|exposure| exposure.name() == port.name())
                else {
                    if carries_usage(port) && !forwarded.is_empty() {
                        continue;
                    }
                    return Some(mismatch(None));
                };
                let alike = self.port(exposure.port_id()).is_some_and(|exposed| {
                    exposed.direction() == port.direction() && exposed.modality() == port.modality()
                });
                if !alike || !matched.insert(exposure.exposure_id()) {
                    return Some(mismatch(Some(exposure.exposure_id())));
                }
            }
            if let Some(exposure) = exposures
                .iter()
                .find(|exposure| !matched.contains(&exposure.exposure_id()))
            {
                return Some(SubgraphInterfaceMismatch {
                    node_id,
                    port_id: None,
                    exposure_id: Some(exposure.exposure_id()),
                });
            }
        }
        None
    }

    /// The nodes of `nodes` on some path from a node of role `source` to a node of role `sink`
    /// along edges of the pipeline whose two ports are both essential ports of its nodes.
    fn on_required_paths(&self, pipeline_id: PipelineId, nodes: &[&Node]) -> BTreeSet<NodeId> {
        let essential = |port_id: PortId| {
            self.port(port_id).filter(|port| {
                port.essentiality() == Essentiality::Essential
                    && nodes.iter().any(|node| node.node_id() == port.node_id())
            })
        };
        let required: Vec<(NodeId, NodeId)> = self
            .edges()
            .filter(|edge| edge.pipeline_id() == pipeline_id)
            .filter_map(|edge| {
                Some((
                    essential(edge.from_port())?.node_id(),
                    essential(edge.to_port())?.node_id(),
                ))
            })
            .collect();
        let with_role = |role: NodeRole| {
            nodes
                .iter()
                .filter(move |node| node.role() == role)
                .map(|node| node.node_id())
        };
        let from_sources = reach(with_role(NodeRole::Source), &required, |(from, to)| {
            (from, to)
        });
        let to_sinks = reach(with_role(NodeRole::Sink), &required, |(from, to)| {
            (to, from)
        });
        from_sources.intersection(&to_sinks).copied().collect()
    }
}

/// The first capability of [`Capability::ALL`] that `required` names and `implemented` lacks.
fn first_unmet(required: &[Capability], implemented: &[Capability]) -> Option<Capability> {
    Capability::ALL
        .into_iter()
        .find(|capability| required.contains(capability) && !implemented.contains(capability))
}

/// Every node `starts` reach along `edges`, the starts included; `direction` reads an edge as the
/// step from its first node to its second.
fn reach(
    starts: impl IntoIterator<Item = NodeId>,
    edges: &[(NodeId, NodeId)],
    direction: impl Fn((NodeId, NodeId)) -> (NodeId, NodeId),
) -> BTreeSet<NodeId> {
    let mut reached = BTreeSet::new();
    let mut pending: Vec<NodeId> = starts.into_iter().collect();
    while let Some(node_id) = pending.pop() {
        if reached.insert(node_id) {
            pending.extend(
                edges
                    .iter()
                    .map(|edge| direction(*edge))
                    .filter(|(from, _)| *from == node_id)
                    .map(|(_, to)| to),
            );
        }
    }
    reached
}
