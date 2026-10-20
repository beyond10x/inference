//! `describe`: what a session runs, read from the graph at the call
//! (`docs/design/inference.md` 718: "selected models, graph, capabilities, and degradation").
//!
//! [`describe`] reads one pipeline of a declaration: its identity and name, its interaction mode,
//! its nodes with the adapter each is bound to, the effective capabilities and the downgrades its
//! plan computed, and its exposures with their direction, capacity and overload policy.
//! [`LiveSession::describe`] answers it for the pipeline the session is over now, so that it
//! follows a switch. It is a read and not a session command: `ess/domains/session.yaml` declares
//! no command for it, and it changes nothing.

use std::num::NonZeroUsize;

use inference_core::id::NodeId;
use inference_core::model::{AdapterId, Capability};
use inference_pipeline::{
    Declaration, ExposureId, InteractionMode, NodeRole, OverloadPolicy, PipelineId, PortDirection,
};

use crate::live::LiveSession;
use crate::session::Session;

/// A node of the described pipeline and the adapter it is bound to, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeDescription {
    pub node_id: NodeId,
    pub name: String,
    pub role: NodeRole,
    pub adapter_id: Option<AdapterId>,
}

/// An exposure of the described pipeline: a named ingress or egress the application reaches it
/// through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExposureDescription {
    pub exposure_id: ExposureId,
    pub name: String,
    /// Input for an ingress, output for an egress; none when the declaration holds no port of
    /// the exposure.
    pub direction: Option<PortDirection>,
    pub capacity: NonZeroUsize,
    pub overload: OverloadPolicy,
}

/// What a pipeline is, as `describe` answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Description {
    pub pipeline_id: PipelineId,
    pub name: String,
    pub mode: InteractionMode,
    /// Every node, in declaration order.
    pub nodes: Vec<NodeDescription>,
    /// The capabilities the plan found on every required path; empty until the pipeline is
    /// planned.
    pub effective_capabilities: Vec<Capability>,
    /// The required capabilities the plan degraded; empty until the pipeline is planned.
    pub downgrades: Vec<Capability>,
    /// Every exposure, in declaration order.
    pub exposures: Vec<ExposureDescription>,
}

/// Describes the pipeline `pipeline_id` of `declaration`; none when the declaration holds no such
/// pipeline.
#[must_use]
pub fn describe(declaration: &Declaration, pipeline_id: PipelineId) -> Option<Description> {
    let pipeline = declaration.pipeline(pipeline_id)?;
    let nodes = declaration
        .nodes()
        .filter(|node| node.pipeline_id() == pipeline_id)
        .map(|node| NodeDescription {
            node_id: node.node_id(),
            name: node.name().to_owned(),
            role: node.role(),
            adapter_id: declaration
                .bindings()
                .find(|binding| {
                    binding.pipeline_id() == pipeline_id && binding.node_id() == node.node_id()
                })
                .map(inference_pipeline::Binding::adapter_id),
        })
        .collect();
    let exposures = declaration
        .exposures()
        .filter(|exposure| exposure.pipeline_id() == pipeline_id)
        .map(|exposure| ExposureDescription {
            exposure_id: exposure.exposure_id(),
            name: exposure.name().to_owned(),
            direction: declaration
                .port(exposure.port_id())
                .map(inference_pipeline::Port::direction),
            capacity: exposure.capacity(),
            overload: exposure.overload(),
        })
        .collect();
    Some(Description {
        pipeline_id,
        name: pipeline.name().to_owned(),
        mode: pipeline.mode(),
        nodes,
        effective_capabilities: pipeline
            .effective_capabilities()
            .map(<[Capability]>::to_vec)
            .unwrap_or_default(),
        downgrades: pipeline
            .downgrades()
            .map(<[Capability]>::to_vec)
            .unwrap_or_default(),
        exposures,
    })
}

impl<P> LiveSession<P> {
    /// `describe`: the pipeline the session is over now, read from `declaration`, the one it was
    /// planned in; none when the declaration holds no such pipeline.
    #[must_use]
    pub fn describe(&self, declaration: &Declaration) -> Option<Description> {
        let pipeline_id = crate::live::lock(&self.shared.sessions)
            .get(self.shared.session_id)
            .map(Session::pipeline_id)?;
        describe(declaration, pipeline_id)
    }
}
