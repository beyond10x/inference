//! The typed builder: a pipeline declared in Rust with compile-time help, into the same
//! [`Declaration`] the commands write.
//!
//! [`Declaration::build`] declares one pipeline and hands a [`PipelineBuilder`] to a closure. A
//! node is a [`NodeHandle`]; a port is a [`PortHandle`] whose direction ([`direction`]) and
//! modality ([`modality`]) are type parameters, so [`PipelineBuilder::connect`] accepts an output
//! port and an input port of one modality and nothing else, and its capacity is a
//! [`NonZeroUsize`]. Every handle is branded with the closure's own lifetime, which no other
//! builder shares and which cannot leave the closure, so a handle names an entity of the pipeline
//! being built or does not compile.
//!
//! The profile and the adapters are named by identity and read through the same [`Catalog`] the
//! commands read, and the builder refuses one the catalog does not hold as the commands do
//! (`unknown-profile`, `unknown-adapter`). What else the commands refuse cannot be written here,
//! so the builder creates through the same inserts the commands reach after their checks, in the
//! order it is called, and a declaration built this way equals one written by the commands with
//! the same input in the same order.

use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroUsize;

use inference_core::model::{AdapterId, Capability, Modality};
use inference_core::policy::{ProfileId, ProfileNotFound};

use crate::{
    AdapterNotFound, AddNode, AddPort, AddSubgraphNodeError, BindingId, Catalog, Declaration,
    DeclarePipeline, Delivery, EdgeId, Essentiality, ExposureId, FeedbackLoopId, InteractionMode,
    NodeId, NodeRole, OverloadPolicy, PipelineId, PortDirection, PortId,
};

/// The lifetime that brands every handle of one builder; invariant, so no other lifetime stands
/// in for it.
type Brand<'b> = PhantomData<fn(&'b ()) -> &'b ()>;

/// The fields of `inference.graph.DeclarePipeline` other than the profile, which
/// [`Declaration::build`] takes by identity and reads through its catalog.
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

/// An exposure of the pipeline being built, of the direction `D` and modality `M` of its port: an
/// ingress when `D` is [`direction::Input`], an egress when it is [`direction::Output`]. It names
/// an entity the builder already declared and is used by no builder method, so unlike a
/// [`PortHandle`] it carries no brand and may leave the closure, telling its holder what the
/// interface carries.
#[derive(Debug)]
pub struct Exposed<D, M> {
    exposure_id: ExposureId,
    kind: PhantomData<fn() -> (D, M)>,
}

impl<D, M> Clone for Exposed<D, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D, M> Copy for Exposed<D, M> {}

impl<D: PortDirectionType, M: PortModality> Exposed<D, M> {
    /// The exposure's identity.
    #[must_use]
    pub const fn id(&self) -> ExposureId {
        self.exposure_id
    }

    /// The direction of the exposed port: input for an ingress, output for an egress.
    #[must_use]
    pub const fn direction(&self) -> PortDirection {
        D::DIRECTION
    }

    /// The modality of the exposed port.
    #[must_use]
    pub const fn modality(&self) -> Modality {
        M::MODALITY
    }
}

/// Declares the nodes, ports, edges, exposures and bindings of one Declared pipeline; see the module
/// documentation.
pub struct PipelineBuilder<'d, 'b> {
    declaration: &'d mut Declaration,
    catalog: &'d dyn Catalog,
    pipeline_id: PipelineId,
    brand: Brand<'b>,
}

impl fmt::Debug for PipelineBuilder<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PipelineBuilder")
            .field("declaration", &self.declaration)
            .field("pipeline_id", &self.pipeline_id)
            .finish_non_exhaustive()
    }
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

    /// Adds a node of role `role` standing for the Planned pipeline `subgraph_id`, as
    /// `inference.graph.AddSubgraphNode`. Its ports are added with [`PipelineBuilder::input`] and
    /// [`PipelineBuilder::output`]; planning holds them to the child's exposures.
    ///
    /// # Errors
    ///
    /// [`AddSubgraphNodeError::UnknownSubgraph`] when the declaration holds no pipeline carrying
    /// `subgraph_id`, [`AddSubgraphNodeError::SubgraphNotPlanned`] when it is not Planned; nothing
    /// is added and no identity is taken.
    pub fn subgraph(
        &mut self,
        name: impl Into<String>,
        role: NodeRole,
        subgraph_id: PipelineId,
    ) -> Result<NodeHandle<'b>, AddSubgraphNodeError> {
        let child = self
            .declaration
            .subgraph_of(subgraph_id)
            .map_err(crate::declaration::SubgraphRefusal::into_add_error)?;
        let added =
            self.declaration
                .insert_subgraph_node(self.pipeline_id, name.into(), role, child);
        Ok(NodeHandle {
            node_id: added.node_id,
            brand: PhantomData,
        })
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
            pipeline_id: self.pipeline_id,
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
    ///
    /// `ConnectPorts` carries its capacity as an ESS `Integer`, so no command writes an edge
    /// holding more than `i64::MAX`; a larger capacity is held as `i64::MAX`, as `ConnectPorts`
    /// holds a capacity beyond `usize::MAX` as `usize::MAX`, so the builder writes only what a
    /// command can write.
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
                carried(capacity),
                overload,
            )
            .edge_id
    }

    /// Connects an output port to an input port as [`PipelineBuilder::connect`] does, and declares
    /// the new edge a feedback edge of the pipeline bounded by `max_iterations`, as
    /// `inference.graph.DeclareFeedbackLoop`: a cycle that runs through it validates (check 5).
    /// Returns the edge's identity and the loop's. A maximum beyond `i64::MAX` is held as
    /// `i64::MAX`, as [`PipelineBuilder::connect`] holds a capacity.
    pub fn connect_feedback<M: PortModality>(
        &mut self,
        from: &PortHandle<'b, direction::Output, M>,
        to: &PortHandle<'b, direction::Input, M>,
        capacity: NonZeroUsize,
        overload: OverloadPolicy,
        max_iterations: NonZeroUsize,
    ) -> (EdgeId, FeedbackLoopId) {
        let edge_id = self.connect(from, to, capacity, overload);
        let feedback_id = self
            .declaration
            .insert_feedback_loop(self.pipeline_id, edge_id, carried(max_iterations))
            .feedback_id;
        (edge_id, feedback_id)
    }

    /// Exposes `port` to the application under `name` with finite buffering, as
    /// `inference.graph.ExposePort`: an input port becomes an ingress, an output port an egress,
    /// and the answer carries the port's direction and modality as types. A capacity beyond
    /// `i64::MAX` is held as `i64::MAX`, as [`PipelineBuilder::connect`] holds one.
    pub fn expose<D: PortDirectionType, M: PortModality>(
        &mut self,
        port: &PortHandle<'b, D, M>,
        name: impl Into<String>,
        capacity: NonZeroUsize,
        overload: OverloadPolicy,
    ) -> Exposed<D, M> {
        let exposure_id = self
            .declaration
            .insert_exposure(
                self.pipeline_id,
                port.port_id,
                name.into(),
                carried(capacity),
                overload,
            )
            .exposure_id;
        Exposed {
            exposure_id,
            kind: PhantomData,
        }
    }

    /// Binds `node` to the adapter carrying `adapter_id`, as `inference.graph.BindNode`.
    ///
    /// # Errors
    ///
    /// [`AdapterNotFound`] when the builder's catalog holds no adapter carrying `adapter_id`;
    /// nothing is bound and no identity is taken.
    pub fn bind(
        &mut self,
        node: &NodeHandle<'b>,
        adapter_id: AdapterId,
    ) -> Result<BindingId, AdapterNotFound> {
        if self.catalog.adapter(adapter_id).is_none() {
            return Err(AdapterNotFound { adapter_id });
        }
        Ok(self
            .declaration
            .insert_binding(self.pipeline_id, node.node_id, adapter_id)
            .binding_id)
    }
}

/// The value a `ConnectPorts` or `DeclareFeedbackLoop` input can carry: `value`, or `i64::MAX`
/// when it is larger.
pub(crate) fn carried(value: NonZeroUsize) -> NonZeroUsize {
    let largest = usize::try_from(i64::MAX).unwrap_or(usize::MAX);
    if value.get() <= largest {
        value
    } else {
        NonZeroUsize::new(largest).unwrap_or(NonZeroUsize::MAX)
    }
}

impl Declaration {
    /// Declares a pipeline under the profile carrying `profile_id`, as
    /// `inference.graph.DeclarePipeline`, and runs `define` with its builder, which binds nodes to
    /// the adapters `catalog` holds. Returns the pipeline's identity and what `define` returned,
    /// which cannot carry a handle.
    ///
    /// ```
    /// use std::num::NonZeroUsize;
    ///
    /// use inference_core::model::{Adapter, AdapterId, Capability};
    /// use inference_core::policy::{
    ///     DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind,
    /// };
    /// use inference_pipeline::{
    ///     Catalog, Declaration, Delivery, Essentiality, InteractionMode, NodeRole, OverloadPolicy,
    ///     PipelineSpec, PortTraits, modality,
    /// };
    ///
    /// /// The one profile of this example, and no adapter.
    /// struct Profiles(DeploymentProfile);
    ///
    /// impl Catalog for Profiles {
    ///     fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
    ///         (self.0.profile_id() == profile_id).then_some(&self.0)
    ///     }
    ///
    ///     fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
    ///         None
    ///     }
    /// }
    ///
    /// let (profile, _) = DeploymentProfile::declare(DeclareProfile {
    ///     kind: ProfileKind::OfflineStrict,
    ///     egress: Egress::Denied,
    ///     admitted: Vec::new(),
    /// })?;
    /// let profile_id = profile.profile_id();
    /// let catalog = Profiles(profile);
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
    /// declaration.build(&catalog, profile_id, spec, |pipeline| {
    ///     let source = pipeline.node("source", NodeRole::Source, Vec::new());
    ///     let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
    ///     let out = pipeline.output::<modality::Text>(&source, "out", traits);
    ///     let into = pipeline.input::<modality::Text>(&sink, "in", traits);
    ///     pipeline.connect(&out, &into, NonZeroUsize::MIN, OverloadPolicy::Pause);
    /// })?;
    /// assert_eq!(declaration.edges().count(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// An edge from an input port, or between two modalities, does not compile:
    ///
    /// ```compile_fail
    /// # use std::num::NonZeroUsize;
    /// # use inference_core::model::{Adapter, AdapterId};
    /// # use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
    /// # use inference_pipeline::{Catalog, Declaration, Delivery, Essentiality, InteractionMode, NodeRole, OverloadPolicy, PipelineSpec, PortTraits, modality};
    /// # struct Profiles(DeploymentProfile);
    /// # impl Catalog for Profiles {
    /// #     fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> { (self.0.profile_id() == profile_id).then_some(&self.0) }
    /// #     fn adapter(&self, _: AdapterId) -> Option<&Adapter> { None }
    /// # }
    /// # let (profile, _) = DeploymentProfile::declare(DeclareProfile { kind: ProfileKind::OfflineStrict, egress: Egress::Denied, admitted: Vec::new() }).unwrap();
    /// # let profile_id = profile.profile_id();
    /// # let catalog = Profiles(profile);
    /// # let traits = PortTraits { essentiality: Essentiality::Essential, delivery: Delivery::Reliable, incremental: true, end_marker_required: true, provisional_allowed: false };
    /// # let spec = PipelineSpec { name: "n".to_owned(), schema_revision: "1".to_owned(), mode: InteractionMode::Batch, required_capabilities: Vec::new() };
    /// let mut declaration = Declaration::new();
    /// declaration.build(&catalog, profile_id, spec, |pipeline| {
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
    /// # use inference_core::model::{Adapter, AdapterId};
    /// # use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
    /// # use inference_pipeline::{Catalog, Declaration, Delivery, Essentiality, InteractionMode, NodeRole, OverloadPolicy, PipelineSpec, PortTraits, modality};
    /// # struct Profiles(DeploymentProfile);
    /// # impl Catalog for Profiles {
    /// #     fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> { (self.0.profile_id() == profile_id).then_some(&self.0) }
    /// #     fn adapter(&self, _: AdapterId) -> Option<&Adapter> { None }
    /// # }
    /// # let (profile, _) = DeploymentProfile::declare(DeclareProfile { kind: ProfileKind::OfflineStrict, egress: Egress::Denied, admitted: Vec::new() }).unwrap();
    /// # let profile_id = profile.profile_id();
    /// # let catalog = Profiles(profile);
    /// # let traits = PortTraits { essentiality: Essentiality::Essential, delivery: Delivery::Reliable, incremental: true, end_marker_required: true, provisional_allowed: false };
    /// # let spec = || PipelineSpec { name: "n".to_owned(), schema_revision: "1".to_owned(), mode: InteractionMode::Batch, required_capabilities: Vec::new() };
    /// let mut first = Declaration::new();
    /// let mut second = Declaration::new();
    /// first.build(&catalog, profile_id, spec(), |outer| {
    ///     let node = outer.node("node", NodeRole::Source, Vec::new());
    ///     second.build(&catalog, profile_id, spec(), |inner| {
    ///         inner.output::<modality::Text>(&node, "out", traits);
    ///     });
    /// });
    /// ```
    ///
    /// # Errors
    ///
    /// [`ProfileNotFound`] when `catalog` holds no profile carrying `profile_id`, as
    /// `DeclarePipeline` refuses it; nothing is declared, `define` does not run and no identity
    /// is taken.
    pub fn build<R>(
        &mut self,
        catalog: &dyn Catalog,
        profile_id: ProfileId,
        pipeline: PipelineSpec,
        define: impl for<'b> FnOnce(&mut PipelineBuilder<'_, 'b>) -> R,
    ) -> Result<(PipelineId, R), ProfileNotFound> {
        if catalog.profile(profile_id).is_none() {
            return Err(ProfileNotFound { profile_id });
        }
        let pipeline_id = self
            .insert_pipeline(DeclarePipeline {
                name: pipeline.name,
                schema_revision: pipeline.schema_revision,
                mode: pipeline.mode,
                profile_id,
                required_capabilities: pipeline.required_capabilities,
            })
            .pipeline_id;
        let mut builder = PipelineBuilder {
            declaration: self,
            catalog,
            pipeline_id,
            brand: PhantomData,
        };
        Ok((pipeline_id, define(&mut builder)))
    }
}
