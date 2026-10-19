//! Adversary pass 1: an edge whose capacity the declaration accepts builds a queue. The capacity is
//! a bound on what the queue may hold (`docs/design/inference.md:679`, invariant `capacity > 0` of
//! `inference.graph.Edge`, no upper bound declared), not an amount of memory taken when the queue
//! is built.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::panic;

use inference_core::id::GenerationId;
use inference_core::model::Capability;
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileKind};
use inference_pipeline::{
    Declaration, Delivery, Edge, Essentiality, InteractionMode, NodeRole, OverloadPolicy,
    PipelineSpec, PortTraits, modality,
};
use inference_runtime::EdgeQueue;

/// One declared edge of a two-node text pipeline, with `capacity` and `overload`.
fn edge(capacity: NonZeroUsize, overload: OverloadPolicy) -> Edge {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let traits = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let spec = PipelineSpec {
        name: "edge".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::Continuous,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let mut declaration = Declaration::new();
    declaration.build(&profile, spec, |pipeline| {
        let source = pipeline.node("source", NodeRole::Source, Vec::new());
        let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
        let out = pipeline.output::<modality::Text>(&source, "out", traits);
        let into = pipeline.input::<modality::Text>(&sink, "in", traits);
        pipeline.connect(&out, &into, capacity, overload);
    });
    declaration.edges().next().expect("one edge").clone()
}

#[test]
fn the_largest_capacity_connect_ports_accepts_builds_a_queue() {
    let largest = usize::try_from(i64::MAX).expect("a 64-bit target");
    let capacity = NonZeroUsize::new(largest).expect("positive");
    let edge = edge(capacity, OverloadPolicy::Pause);
    let built = panic::catch_unwind(|| {
        EdgeQueue::<u32>::new(&edge, HashMap::<GenerationId, i64>::new()).map(|queue| {
            let (sender, receiver) = queue.split();
            (sender.len(), receiver.len())
        })
    });
    assert!(
        built.is_ok(),
        "EdgeQueue::new panicked for a declared capacity of {largest}"
    );
}
