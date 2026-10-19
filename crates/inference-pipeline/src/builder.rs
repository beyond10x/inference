//! The typed builder: a pipeline declared in Rust with compile-time help, into the same
//! [`Declaration`] the commands write.
//!
//! [`Declaration::build`] declares one pipeline and hands a [`PipelineBuilder`] to a closure. A
//! node is a [`NodeHandle`]; a port is a [`PortHandle`] whose direction ([`direction`]) and
//! modality ([`modality`]) are type parameters, so [`PipelineBuilder::connect`] accepts an output
//! port and an input port of one modality and nothing else, and its capacity is a
//! [`NonZeroUsize`]. A node is bound to an [`Adapter`] value, not an identity. Every handle is
//! branded with the closure's own lifetime, which no other builder shares and which cannot leave
//! the closure, so a handle names an entity of the pipeline being built or does not compile.
//!
//! What the commands refuse cannot be written here, so the builder creates through the same
//! inserts the commands reach after their checks, in the order it is called, and a declaration
//! built this way equals one written by the commands with the same input in the same order.

use std::marker::PhantomData;
use std::num::NonZeroUsize;

use inference_core::model::{Adapter, Capability, Modality};
use inference_core::policy::DeploymentProfile;

use crate::{
    AddNode, AddPort, BindingId, Declaration, DeclarePipeline, Delivery, EdgeId, Essentiality,
    InteractionMode, NodeId, NodeRole, OverloadPolicy, PipelineId, PortDirection, PortId,
};

/// The lifetime that brands every handle of one builder; invariant, so no other lifetime stands
/// in for it.
type Brand<'b> = PhantomData<fn(&'b ()) -> &'b ()>;

/// The fields of `inference.graph.DeclarePipeline` other than the profile, which the builder takes
/// as a [`DeploymentProfile`] value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineSpec {
    pub name: String,
    pub schema_revision: String,
    pub mode: InteractionMode,
    pub required_capabilities: Vec<Capability>,
}

/// The fields of `inference.graph.AddPort` other than the node, the name, the direction and the
/// modality, which the builder takes from its handle and type parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortTraits {
    pub essentiality: Essentiality,
    pub delivery: Delivery,
    pub incremental: bool,
    pub end_marker_required: bool,
    pub provisional_allowed: bool,
}

/// An essential, reliable port of whole frames with no end marker that admits committed data only.
/// Committed only is the default of every port and so of a tool's input ports, since a
/// side-effecting tool requires committed input by default (§11.3); a port that admits
/// provisional data says so explicitly.
impl Default for PortTraits {
    fn default() -> Self {
        Self {
            essentiality: Essentiality::Essential,
            delivery: Delivery::Reliable,
            incremental: false,
            end_marker_required: false,
            provisional_allowed: false,
        }
    }
}

/// A port direction as a type: [`direction::Input`] or [`direction::Output`].
pub trait PortDirectionType {
    /// The direction the type stands for.
    const DIRECTION: PortDirection;
}

/// A modality as a type, one per member of [`Modality`]: [`modality::Text`], [`modality::Audio`]
/// and so on.
pub trait PortModality {
    /// The modality the type stands for.
    const MODALITY: Modality;
}

/// The port directions as types.
pub mod direction {
    use super::PortDirectionType;
    use crate::PortDirection;

    /// An input port.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Input {}

    /// An output port.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Output {}

    impl PortDirectionType for Input {
        const DIRECTION: PortDirection = PortDirection::Input;
    }

    impl PortDirectionType for Output {
        const DIRECTION: PortDirection = PortDirection::Output;
    }
}

/// The modalities of `inference.model.Modality` as types.
pub mod modality {
    use inference_core::model::Modality;

    use super::PortModality;

    macro_rules! modality_type {
        ($($name:ident),+) => {$(
            #[doc = concat!("[`Modality::", stringify!($name), "`] as a type.")]
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub enum $name {}

            impl PortModality for $name {
                const MODALITY: Modality = Modality::$name;
            }
        )+};
    }

    modality_type!(Text, Audio, Structured, Image, Video, Control);
}

/// A node of the pipeline being built.
#[derive(Debug)]
pub struct NodeHandle<'b> {
    node_id: NodeId,
    brand: Brand<'b>,
}

impl NodeHandle<'_> {
    /// The node's identity.
    #[must_use]
    pub const fn id(&self) -> NodeId {
        self.node_id
    }
}

/// A port of the pipeline being built, of direction `D` and modality `M`.
#[derive(Debug)]
pub struct PortHandle<'b, D, M> {
    port_id: PortId,
    brand: Brand<'b>,
    kind: PhantomData<fn() -> (D, M)>,
}

impl<D: PortDirectionType, M: PortModality> PortHandle<'_, D, M> {
    /// The port's identity.
    #[must_use]
    pub const fn id(&self) -> PortId {
        self.port_id
    }

    /// The port's direction.
    #[must_use]
    pub const fn direction(&self) -> PortDirection {
        D::DIRECTION
    }

    /// The port's modality.
    #[must_use]
    pub const fn modality(&self) -> Modality {
        M::MODALITY
    }
}

/// Declares the nodes, ports, edges and bindings of one Declared pipeline; see the module
/// documentation.
#[derive(Debug)]
pub struct PipelineBuilder<'d, 'b> {
    declaration: &'d mut Declaration,
    pipeline_id: PipelineId,
    brand: Brand<'b>,
}

impl<'b> PipelineBuilder<'_, 'b> {
    /// The pipeline being built.
    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    /// Adds a node, as `inference.graph.AddNode`.
    pub fn node(
        &mut self,
        name: impl Into<String>,
        role: NodeRole,
        requires: Vec<Capability>,
    ) -> NodeHandle<'b> {
        let added = self.declaration.insert_node(AddNode {
            pipeline_id: self.pipeline_id,
            name: name.into(),
            role,
            requires,
        });
        NodeHandle {
            node_id: added.node_id,
            brand: PhantomData,
        }
    }

    /// Adds an input port of modality `M` to `node`, as `inference.graph.AddPort`.
    pub fn input<M: PortModality>(
        &mut self,
        node: &NodeHandle<'b>,
        name: impl Into<String>,
        traits: PortTraits,
    ) -> PortHandle<'b, direction::Input, M> {
        self.port(node, name.into(), traits)
    }

    /// Adds an output port of modality `M` to `node`, as `inference.graph.AddPort`.
    pub fn output<M: PortModality>(
        &mut self,
        node: &NodeHandle<'b>,
        name: impl Into<String>,
        traits: PortTraits,
    ) -> PortHandle<'b, direction::Output, M> {
        self.port(node, name.into(), traits)
    }

    fn port<D: PortDirectionType, M: PortModality>(
        &mut self,
        node: &NodeHandle<'b>,
        name: String,
        traits: PortTraits,
    ) -> PortHandle<'b, D, M> {
        let added = self.declaration.insert_port(AddPort {
            node_id: node.node_id,
            name,
            direction: D::DIRECTION,
            modality: M::MODALITY,
            essentiality: traits.essentiality,
            delivery: traits.delivery,
            incremental: traits.incremental,
            end_marker_required: traits.end_marker_required,
            provisional_allowed: traits.provisional_allowed,
        });
        PortHandle {
            port_id: added.port_id,
            brand: PhantomData,
            kind: PhantomData,
        }
    }

    /// Connects an output port to an input port of the same modality with finite buffering, as
    /// `inference.graph.ConnectPorts`.
    pub fn connect<M: PortModality>(
        &mut self,
        from: &PortHandle<'b, direction::Output, M>,
        to: &PortHandle<'b, direction::Input, M>,
        capacity: NonZeroUsize,
        overload: OverloadPolicy,
    ) -> EdgeId {
        self.declaration
            .insert_edge(
                self.pipeline_id,
                from.port_id,
                to.port_id,
                capacity,
                overload,
            )
            .edge_id
    }

    /// Binds `node` to `adapter`, as `inference.graph.BindNode`.
    pub fn bind(&mut self, node: &NodeHandle<'b>, adapter: &Adapter) -> BindingId {
        self.declaration
            .insert_binding(self.pipeline_id, node.node_id, adapter.adapter_id())
            .binding_id
    }
}

impl Declaration {
    /// Declares a pipeline under `profile`, as `inference.graph.DeclarePipeline`, and runs
    /// `define` with its builder. Returns the pipeline's identity and what `define` returned,
    /// which cannot carry a handle.
    ///
    /// ```
    /// use std::num::NonZeroUsize;
    ///
    /// use inference_core::model::Capability;
    /// use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileKind};
    /// use inference_pipeline::{
    ///     Declaration, Delivery, Essentiality, InteractionMode, NodeRole, OverloadPolicy,
    ///     PipelineSpec, PortTraits, modality,
    /// };
    ///
    /// let (profile, _) = DeploymentProfile::declare(DeclareProfile {
    ///     kind: ProfileKind::OfflineStrict,
    ///     egress: Egress::Denied,
    ///     admitted: Vec::new(),
    /// })?;
    /// let traits = PortTraits {
    ///     essentiality: Essentiality::Essential,
    ///     delivery: Delivery::Reliable,
    ///     incremental: true,
    ///     end_marker_required: true,
    ///     provisional_allowed: false,
    /// };
    /// let spec = PipelineSpec {
    ///     name: "echo".to_owned(),
    ///     schema_revision: "1".to_owned(),
    ///     mode: InteractionMode::Batch,
    ///     required_capabilities: vec![Capability::OutputStreaming],
    /// };
    /// let mut declaration = Declaration::new();
    /// declaration.build(&profile, spec, |pipeline| {
    ///     let source = pipeline.node("source", NodeRole::Source, Vec::new());
    ///     let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
    ///     let out = pipeline.output::<modality::Text>(&source, "out", traits);
    ///     let into = pipeline.input::<modality::Text>(&sink, "in", traits);
    ///     pipeline.connect(&out, &into, NonZeroUsize::MIN, OverloadPolicy::Pause);
    /// });
    /// assert_eq!(declaration.edges().count(), 1);
    /// # Ok::<(), inference_core::policy::DeclareProfileError>(())
    /// ```
    ///
    /// An edge from an input port, or between two modalities, does not compile:
    ///
    /// ```compile_fail
    /// # use std::num::NonZeroUsize;
    /// # use inference_core::model::Capability;
    /// # use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileKind};
    /// # use inference_pipeline::{Declaration, Delivery, Essentiality, InteractionMode, NodeRole, OverloadPolicy, PipelineSpec, PortTraits, modality};
    /// # let (profile, _) = DeploymentProfile::declare(DeclareProfile { kind: ProfileKind::OfflineStrict, egress: Egress::Denied, admitted: Vec::new() }).unwrap();
    /// # let traits = PortTraits { essentiality: Essentiality::Essential, delivery: Delivery::Reliable, incremental: true, end_marker_required: true, provisional_allowed: false };
    /// # let spec = PipelineSpec { name: "n".to_owned(), schema_revision: "1".to_owned(), mode: InteractionMode::Batch, required_capabilities: Vec::new() };
    /// let mut declaration = Declaration::new();
    /// declaration.build(&profile, spec, |pipeline| {
    ///     let node = pipeline.node("node", NodeRole::Transform, Vec::new());
    ///     let text = pipeline.input::<modality::Text>(&node, "text", traits);
    ///     let audio = pipeline.output::<modality::Audio>(&node, "audio", traits);
    ///     pipeline.connect(&audio, &text, NonZeroUsize::MIN, OverloadPolicy::Pause);
    /// });
    /// ```
    ///
    /// A handle of one builder is not accepted by another:
    ///
    /// ```compile_fail
    /// # use std::num::NonZeroUsize;
    /// # use inference_core::model::Capability;
    /// # use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileKind};
    /// # use inference_pipeline::{Declaration, Delivery, Essentiality, InteractionMode, NodeRole, OverloadPolicy, PipelineSpec, PortTraits, modality};
    /// # let (profile, _) = DeploymentProfile::declare(DeclareProfile { kind: ProfileKind::OfflineStrict, egress: Egress::Denied, admitted: Vec::new() }).unwrap();
    /// # let traits = PortTraits { essentiality: Essentiality::Essential, delivery: Delivery::Reliable, incremental: true, end_marker_required: true, provisional_allowed: false };
    /// # let spec = || PipelineSpec { name: "n".to_owned(), schema_revision: "1".to_owned(), mode: InteractionMode::Batch, required_capabilities: Vec::new() };
    /// let mut first = Declaration::new();
    /// let mut second = Declaration::new();
    /// first.build(&profile, spec(), |outer| {
    ///     let node = outer.node("node", NodeRole::Source, Vec::new());
    ///     second.build(&profile, spec(), |inner| {
    ///         inner.output::<modality::Text>(&node, "out", traits);
    ///     });
    /// });
    /// ```
    pub fn build<R>(
        &mut self,
        profile: &DeploymentProfile,
        pipeline: PipelineSpec,
        define: impl for<'b> FnOnce(&mut PipelineBuilder<'_, 'b>) -> R,
    ) -> (PipelineId, R) {
        let pipeline_id = self
            .insert_pipeline(DeclarePipeline {
                name: pipeline.name,
                schema_revision: pipeline.schema_revision,
                mode: pipeline.mode,
                profile_id: profile.profile_id(),
                required_capabilities: pipeline.required_capabilities,
            })
            .pipeline_id;
        let mut builder = PipelineBuilder {
            declaration: self,
            pipeline_id,
            brand: PhantomData,
        };
        (pipeline_id, define(&mut builder))
    }
}
