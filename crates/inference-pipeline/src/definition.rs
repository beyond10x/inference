//! Declarative graph definitions (`docs/design/inference.md` §9.3, 491): a pipeline written as
//! JSON, checked against the runtime node descriptors before anything is declared, then declared
//! into a [`Declaration`] through the inserts the commands and the typed builder reach.
//!
//! A [`NodeDescriptors`] holds the [`NodeDescriptor`]s published by `inference.graph.
//! DeclareNodeDescriptor`, one per [`NodeKind`]. A [`PipelineDefinition`] names its profile, its
//! nodes by name and kind, its edges and exposures by node and port name, its feedback loops by the
//! ends of their edge, and its bindings by node name. A definition node does not restate its
//! ports: the declared ports are its descriptor's, and its role and required capabilities are the
//! descriptor's too.
//!
//! [`Declaration::declare_definition`] first checks the whole definition and refuses it with every
//! [`DefinitionFinding`] when a node names an unknown kind or another node's name, an edge,
//! exposure, feedback loop or binding names a node the definition lacks or a port its descriptor
//! lacks, an edge leaves an input port or enters an output port, an edge joins two modalities, or a
//! feedback loop names no edge of the definition. A port is named by its name, and an edge or a
//! feedback loop leaves an output and enters an input, so a descriptor may give an input and an
//! output one name; an exposure, which says no direction, naming such a name is refused
//! (`ambiguous_port`). It then refuses an unknown profile or adapter, as `DeclarePipeline` and
//! `BindNode` do. Only then does it declare, as the builder does, holding a capacity beyond
//! `i64::MAX` as `i64::MAX`, so a refused definition leaves the declaration holding nothing it did
//! not hold before. What the check leaves to the graph,
//! such as commitment semantics, cycles and reachability, is §10.2 validation's, one code path
//! for definitions, the builder and the commands (open question 4, 1229).
//!
//! JSON is the one format: YAML and TOML each need a dependency and are a later step.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};

use inference_core::model::{AdapterId, Capability};
use inference_core::policy::{ProfileId, ProfileNotFound};

use crate::builder::carried;
use crate::{
    AdapterNotFound, AddNode, AddPort, Catalog, Declaration, DeclareNodeDescriptor,
    DeclarePipeline, EdgeId, InteractionMode, NodeDescriptor, NodeDescriptorDeclared,
    NodeDescriptorExists, NodeId, NodeKind, OverloadPolicy, PipelineId, PortDescriptor,
    PortDirection, PortId,
};

/// The node descriptors published so far, one per kind, in declaration order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeDescriptors {
    descriptors: Vec<NodeDescriptor>,
}

impl NodeDescriptors {
    /// No descriptor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every descriptor of `declared`, each published as [`NodeDescriptors::declare`] does.
    ///
    /// # Errors
    ///
    /// [`NodeDescriptorExists`] naming the first kind declared twice; nothing is kept then.
    pub fn of(
        declared: impl IntoIterator<Item = DeclareNodeDescriptor>,
    ) -> Result<Self, NodeDescriptorExists> {
        let mut descriptors = Self::new();
        for input in declared {
            descriptors.declare(input)?;
        }
        Ok(descriptors)
    }

    /// Runs `inference.graph.DeclareNodeDescriptor`: the descriptor is published under its kind.
    ///
    /// # Errors
    ///
    /// The `already-declared` outcome, [`NodeDescriptorExists`], when a descriptor is already
    /// declared under the kind; nothing changes.
    pub fn declare(
        &mut self,
        input: DeclareNodeDescriptor,
    ) -> Result<NodeDescriptorDeclared, NodeDescriptorExists> {
        if self.descriptor(&input.kind).is_some() {
            return Err(NodeDescriptorExists { kind: input.kind });
        }
        let declared = NodeDescriptorDeclared {
            kind: input.kind.clone(),
            role: input.role,
        };
        self.descriptors.push(NodeDescriptor {
            kind: input.kind,
            role: input.role,
            requires: input.requires,
            ports: input.ports,
        });
        Ok(declared)
    }

    /// The descriptor published under `kind`, if one is.
    #[must_use]
    pub fn descriptor(&self, kind: &NodeKind) -> Option<&NodeDescriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.kind() == kind)
    }

    /// Every descriptor, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &NodeDescriptor> {
        self.descriptors.iter()
    }
}

/// A pipeline written declaratively: what `DeclarePipeline` takes, and its nodes, edges,
/// exposures, feedback loops and bindings by name. A field the definition does not know is an
/// error, as is a capacity or an iteration maximum below one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineDefinition {
    pub name: String,
    pub schema_revision: String,
    pub mode: InteractionMode,
    pub profile_id: ProfileId,
    #[serde(default)]
    pub required_capabilities: Vec<Capability>,
    pub nodes: Vec<NodeDefinition>,
    #[serde(default)]
    pub edges: Vec<EdgeDefinition>,
    #[serde(default)]
    pub exposures: Vec<ExposureDefinition>,
    #[serde(default)]
    pub feedback_loops: Vec<FeedbackLoopDefinition>,
    #[serde(default)]
    pub bindings: Vec<BindingDefinition>,
}

/// A node of a definition: its name, unique in the definition, and the kind whose descriptor
/// gives its role, requirements and ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeDefinition {
    pub name: String,
    pub kind: NodeKind,
}

/// A port of a definition, named by its node's name and the port's name in that node's
/// descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortReference {
    pub node: String,
    pub port: String,
}

/// An edge of a definition, from an output port to an input port, with finite buffering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeDefinition {
    pub from: PortReference,
    pub to: PortReference,
    pub capacity: NonZeroUsize,
    pub overload: OverloadPolicy,
}

/// An exposure of a definition: a port exposed to the application under `name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposureDefinition {
    pub name: String,
    pub port: PortReference,
    pub capacity: NonZeroUsize,
    pub overload: OverloadPolicy,
}

/// A feedback loop of a definition: the first edge of the definition running from `from` to `to`
/// is its feedback edge, bounded by `max_iterations`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackLoopDefinition {
    pub from: PortReference,
    pub to: PortReference,
    pub max_iterations: NonZeroUsize,
}

/// A binding of a definition: the node named `node` bound to the adapter `adapter_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingDefinition {
    pub node: String,
    pub adapter_id: AdapterId,
}

/// What the check of a definition against the node descriptors found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionDefect {
    /// No descriptor is published under the node's kind.
    UnknownKind,
    /// Another node of the definition carries the same name.
    DuplicateNode,
    /// An edge, exposure, feedback loop or binding names a node the definition lacks.
    UnknownNode,
    /// An edge, exposure or feedback loop names a port the node's descriptor lacks.
    UnknownPort,
    /// An exposure names a port by a name an input and an output of the node's descriptor share,
    /// so which one it exposes is not said; an edge or a feedback loop is told by its direction.
    AmbiguousPort,
    /// An edge leaves an input port.
    EdgeFromInput,
    /// An edge enters an output port.
    EdgeIntoOutput,
    /// An edge joins ports of two modalities.
    ModalityMismatch,
    /// A feedback loop names two ports no edge of the definition runs between.
    UnknownEdge,
}

/// One finding of the check: the defect, the node it is at, by name, and the port, by name, when
/// it is at one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionFinding {
    pub defect: DefinitionDefect,
    pub node: String,
    pub port: Option<String>,
}

/// Why [`Declaration::declare_definition`] declared nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionError {
    /// The check against the node descriptors found something; every finding is named.
    Rejected(Vec<DefinitionFinding>),
    /// The catalog holds no profile carrying the definition's `profile_id`.
    UnknownProfile(ProfileNotFound),
    /// The catalog holds no adapter carrying a binding's `adapter_id`.
    UnknownAdapter(AdapterNotFound),
}

impl fmt::Display for DefinitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(findings) => write!(
                f,
                "the definition does not match its node descriptors: {findings:?}"
            ),
            Self::UnknownProfile(error) => fmt::Display::fmt(error, f),
            Self::UnknownAdapter(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for DefinitionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Rejected(_) => None,
            Self::UnknownProfile(error) => Some(error),
            Self::UnknownAdapter(error) => Some(error),
        }
    }
}

/// A definition that is not JSON of the shape [`PipelineDefinition`] reads.
#[derive(Debug)]
pub struct DefinitionNotRead(serde_json::Error);

impl fmt::Display for DefinitionNotRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the definition is not read: {}", self.0)
    }
}

impl Error for DefinitionNotRead {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

impl PipelineDefinition {
    /// The definition `json` writes.
    ///
    /// # Errors
    ///
    /// [`DefinitionNotRead`] when `json` is not JSON of this shape: a field missing or unknown, a
    /// name that is not an ESS name, a capacity or maximum below one.
    pub fn from_json(json: &str) -> Result<Self, DefinitionNotRead> {
        serde_json::from_str(json).map_err(DefinitionNotRead)
    }
}

/// What a declared definition became: its pipeline and the identity of each of its nodes, in the
/// definition's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredDefinition {
    pub pipeline_id: PipelineId,
    pub nodes: Vec<(String, NodeId)>,
}

impl DeclaredDefinition {
    /// The identity the node named `name` was declared under.
    #[must_use]
    pub fn node(&self, name: &str) -> Option<NodeId> {
        self.nodes
            .iter()
            .find(|(node, _)| node == name)
            .map(|(_, node_id)| *node_id)
    }
}

impl Declaration {
    /// Checks `definition` against `descriptors` and declares it; see the module documentation.
    ///
    /// # Errors
    ///
    /// In this order: [`DefinitionError::Rejected`] carrying every finding of the check;
    /// [`DefinitionError::UnknownProfile`]; [`DefinitionError::UnknownAdapter`] for the first
    /// binding whose adapter the catalog lacks. Nothing is declared and no identity is taken.
    pub fn declare_definition(
        &mut self,
        definition: &PipelineDefinition,
        descriptors: &NodeDescriptors,
        catalog: &dyn Catalog,
    ) -> Result<DeclaredDefinition, DefinitionError> {
        let checked = Checked::of(definition, descriptors);
        if !checked.findings.is_empty() {
            return Err(DefinitionError::Rejected(checked.findings));
        }
        if catalog.profile(definition.profile_id).is_none() {
            return Err(DefinitionError::UnknownProfile(ProfileNotFound {
                profile_id: definition.profile_id,
            }));
        }
        if let Some(binding) = definition
            .bindings
            .iter()
            .find(|binding| catalog.adapter(binding.adapter_id).is_none())
        {
            return Err(DefinitionError::UnknownAdapter(AdapterNotFound {
                adapter_id: binding.adapter_id,
            }));
        }
        Ok(self.declare_checked(definition, &checked))
    }

    /// Declares a definition the check passed, in the order the typed builder would: the
    /// pipeline, each node followed by its descriptor's ports, the edges, the exposures, the
    /// feedback loops, the bindings.
    fn declare_checked(
        &mut self,
        definition: &PipelineDefinition,
        checked: &Checked<'_>,
    ) -> DeclaredDefinition {
        let pipeline_id = self
            .insert_pipeline(DeclarePipeline {
                name: definition.name.clone(),
                schema_revision: definition.schema_revision.clone(),
                mode: definition.mode,
                profile_id: definition.profile_id,
                required_capabilities: definition.required_capabilities.clone(),
            })
            .pipeline_id;
        let mut nodes = Vec::new();
        let mut ports: Vec<(&str, &PortDescriptor, PortId)> = Vec::new();
        for (node, descriptor) in &checked.nodes {
            let node_id = self
                .insert_node(AddNode {
                    pipeline_id,
                    name: node.name.clone(),
                    role: descriptor.role(),
                    requires: descriptor.requires().to_vec(),
                })
                .node_id;
            for port in descriptor.ports() {
                let port_id = self
                    .insert_port(AddPort {
                        pipeline_id,
                        node_id,
                        name: port.name.clone(),
                        direction: port.direction,
                        modality: port.modality,
                        essentiality: port.essentiality,
                        delivery: port.delivery,
                        incremental: port.incremental,
                        end_marker_required: port.end_marker_required,
                        provisional_allowed: port.provisional_allowed,
                    })
                    .port_id;
                ports.push((node.name.as_str(), port, port_id));
            }
            nodes.push((node.name.clone(), node_id));
        }
        let port_id = |reference: &PortReference, direction: Option<PortDirection>| {
            let of_node: Vec<&PortDescriptor> = ports
                .iter()
                .filter(|(node, _, _)| *node == reference.node)
                .map(|(_, port, _)| *port)
                .collect();
            let chosen = named(&of_node, &reference.port, direction)?;
            ports
                .iter()
                .find(|(node, port, _)| *node == reference.node && std::ptr::eq(*port, chosen))
                .map(|(_, _, port_id)| *port_id)
        };
        let mut edges: Vec<(&EdgeDefinition, EdgeId)> = Vec::new();
        for edge in &definition.edges {
            if let (Some(from), Some(to)) = (
                port_id(&edge.from, Some(PortDirection::Output)),
                port_id(&edge.to, Some(PortDirection::Input)),
            ) {
                let edge_id = self
                    .insert_edge(pipeline_id, from, to, carried(edge.capacity), edge.overload)
                    .edge_id;
                edges.push((edge, edge_id));
            }
        }
        for exposure in &definition.exposures {
            if let Some(port) = port_id(&exposure.port, None) {
                self.insert_exposure(
                    pipeline_id,
                    port,
                    exposure.name.clone(),
                    carried(exposure.capacity),
                    exposure.overload,
                );
            }
        }
        for feedback in &definition.feedback_loops {
            if let Some((_, edge_id)) = edges
                .iter()
                .find(|(edge, _)| edge.from == feedback.from && edge.to == feedback.to)
            {
                self.insert_feedback_loop(pipeline_id, *edge_id, feedback.max_iterations);
            }
        }
        for binding in &definition.bindings {
            if let Some((_, node_id)) = nodes.iter().find(|(name, _)| *name == binding.node) {
                self.insert_binding(pipeline_id, *node_id, binding.adapter_id);
            }
        }
        DeclaredDefinition { pipeline_id, nodes }
    }
}

/// A definition checked against the descriptors: each node with its descriptor, first of its
/// name, and the findings.
struct Checked<'d> {
    nodes: Vec<(&'d NodeDefinition, &'d NodeDescriptor)>,
    findings: Vec<DefinitionFinding>,
}

/// What a port reference resolved to.
#[derive(Clone, Copy)]
enum Resolved<'d> {
    /// The port of the descriptor of the node named.
    Port(&'d crate::PortDescriptor),
    /// The reference names no node, a node of unknown kind, or a port the descriptor lacks; the
    /// finding is recorded where it applies.
    Missing,
}

impl<'d> Checked<'d> {
    /// The findings over `definition`, in this order: the nodes, the edges, the exposures, the
    /// feedback loops and the bindings, each in the definition's order.
    fn of(definition: &'d PipelineDefinition, descriptors: &'d NodeDescriptors) -> Self {
        let mut checked = Self {
            nodes: Vec::new(),
            findings: Vec::new(),
        };
        let mut names = BTreeSet::new();
        let mut unknown_kinds = BTreeSet::new();
        for node in &definition.nodes {
            if !names.insert(node.name.as_str()) {
                checked.find(DefinitionDefect::DuplicateNode, &node.name, None);
                continue;
            }
            match descriptors.descriptor(&node.kind) {
                Some(descriptor) => checked.nodes.push((node, descriptor)),
                None => {
                    unknown_kinds.insert(node.name.as_str());
                    checked.find(DefinitionDefect::UnknownKind, &node.name, None);
                }
            }
        }
        for edge in &definition.edges {
            let from = checked.resolve(&edge.from, Some(PortDirection::Output), &unknown_kinds);
            let to = checked.resolve(&edge.to, Some(PortDirection::Input), &unknown_kinds);
            if let Resolved::Port(source) = from
                && source.direction != PortDirection::Output
            {
                checked.find_at(DefinitionDefect::EdgeFromInput, &edge.from);
            }
            if let Resolved::Port(sink) = to
                && sink.direction != PortDirection::Input
            {
                checked.find_at(DefinitionDefect::EdgeIntoOutput, &edge.to);
            }
            if let (Resolved::Port(source), Resolved::Port(sink)) = (from, to)
                && source.modality != sink.modality
            {
                checked.find_at(DefinitionDefect::ModalityMismatch, &edge.from);
            }
        }
        for exposure in &definition.exposures {
            checked.resolve(&exposure.port, None, &unknown_kinds);
        }
        for feedback in &definition.feedback_loops {
            let from = checked.resolve(&feedback.from, Some(PortDirection::Output), &unknown_kinds);
            let to = checked.resolve(&feedback.to, Some(PortDirection::Input), &unknown_kinds);
            let edge = definition
                .edges
                .iter()
                .any(|edge| edge.from == feedback.from && edge.to == feedback.to);
            if matches!((from, to), (Resolved::Port(_), Resolved::Port(_))) && !edge {
                checked.find_at(DefinitionDefect::UnknownEdge, &feedback.from);
            }
        }
        for binding in &definition.bindings {
            if !names.contains(binding.node.as_str()) {
                checked.find(DefinitionDefect::UnknownNode, &binding.node, None);
            }
        }
        checked
    }

    fn find(&mut self, defect: DefinitionDefect, node: &str, port: Option<&str>) {
        self.findings.push(DefinitionFinding {
            defect,
            node: node.to_owned(),
            port: port.map(str::to_owned),
        });
    }

    fn find_at(&mut self, defect: DefinitionDefect, reference: &PortReference) {
        self.find(defect, &reference.node, Some(&reference.port));
    }

    /// The port `reference` names, of `direction` when one carries the name in it (see
    /// [`named`]), recording `unknown_node`, `unknown_port` or `ambiguous_port` when it names none;
    /// a node of unknown kind was named already, and its ports are not judged again.
    fn resolve(
        &mut self,
        reference: &PortReference,
        direction: Option<PortDirection>,
        unknown_kinds: &BTreeSet<&str>,
    ) -> Resolved<'d> {
        if unknown_kinds.contains(reference.node.as_str()) {
            return Resolved::Missing;
        }
        let Some((_, descriptor)) = self
            .nodes
            .iter()
            .find(|(node, _)| node.name == reference.node)
        else {
            self.find_at(DefinitionDefect::UnknownNode, reference);
            return Resolved::Missing;
        };
        let ports: Vec<&'d PortDescriptor> = descriptor.ports().iter().collect();
        if let Some(port) = named(&ports, &reference.port, direction) {
            return Resolved::Port(port);
        }
        let defect = if ports.iter().any(|port| port.name == reference.port) {
            DefinitionDefect::AmbiguousPort
        } else {
            DefinitionDefect::UnknownPort
        };
        self.find_at(defect, reference);
        Resolved::Missing
    }
}

/// The port of `ports` named `name`: with a `direction`, the one of that direction, else the
/// first of the name, so that a reference to the wrong direction is judged as such; with none, the
/// one port of the name, none when an input and an output share it. A descriptor may name an
/// input and an output alike, as the typed builder may.
fn named<'p>(
    ports: &[&'p PortDescriptor],
    name: &str,
    direction: Option<PortDirection>,
) -> Option<&'p PortDescriptor> {
    let mut of_name = ports.iter().copied().filter(|port| port.name == name);
    match direction {
        Some(direction) => ports
            .iter()
            .copied()
            .find(|port| port.name == name && port.direction == direction)
            .or_else(|| of_name.next()),
        None => {
            let first = of_name.next()?;
            of_name.next().is_none().then_some(first)
        }
    }
}
