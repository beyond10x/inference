//! The node descriptors of the testkit's mock nodes of a text-only pipeline, as
//! `inference.graph.DeclareNodeDescriptor` publishes them (`docs/design/inference.md` 473-482,
//! 491): the text source ([`TEXT_SOURCE`]), the text sink ([`TEXT_SINK`]) and the mock model node
//! of the `llm` feature ([`MODEL`]), so that a declarative definition of the A-02 pipeline is
//! checked against them.
//!
//! Every port carries text incrementally with an end marker, essential, reliable, committed only,
//! as the text-only pipeline declares them through the typed builder. The source sends on `text`,
//! the model takes `prompt` and sends `answer` and requires `output_streaming`, the sink takes
//! `text`.

use inference_core::model::{Capability, Modality};
use inference_pipeline::{
    DeclareNodeDescriptor, Delivery, Essentiality, NodeKind, NodeRole, PortDescriptor,
    PortDirection, PortTraits,
};

/// The kind of [`TextSource`](crate::graph::TextSource).
pub const TEXT_SOURCE: &str = "testkit.text_source";
/// The kind of [`TextSink`](crate::graph::TextSink).
pub const TEXT_SINK: &str = "testkit.text_sink";
/// The kind of the mock model node of a text-only pipeline (`ModelNode` of the `llm` feature).
pub const MODEL: &str = "testkit.model";

/// The traits of every port of the text-only pipeline.
pub const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

fn text(name: &str, direction: PortDirection) -> PortDescriptor {
    PortDescriptor::new(name, direction, Modality::Text, TEXT)
}

/// The text source: role `source`, `text` out.
#[must_use]
pub fn text_source() -> DeclareNodeDescriptor {
    DeclareNodeDescriptor {
        kind: NodeKind::new(TEXT_SOURCE),
        role: NodeRole::Source,
        requires: Vec::new(),
        ports: vec![text("text", PortDirection::Output)],
    }
}

/// The text sink: role `sink`, `text` in.
#[must_use]
pub fn text_sink() -> DeclareNodeDescriptor {
    DeclareNodeDescriptor {
        kind: NodeKind::new(TEXT_SINK),
        role: NodeRole::Sink,
        requires: Vec::new(),
        ports: vec![text("text", PortDirection::Input)],
    }
}

/// The mock model node: role `model`, requiring `output_streaming`, `prompt` in, `answer` out.
#[must_use]
pub fn model() -> DeclareNodeDescriptor {
    DeclareNodeDescriptor {
        kind: NodeKind::new(MODEL),
        role: NodeRole::Model,
        requires: vec![Capability::OutputStreaming],
        ports: vec![
            text("prompt", PortDirection::Input),
            text("answer", PortDirection::Output),
        ],
    }
}

/// The three descriptors, in the order the module lists them.
#[must_use]
pub fn all() -> Vec<DeclareNodeDescriptor> {
    vec![text_source(), text_sink(), model()]
}
