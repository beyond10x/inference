//! Adversary pass 1 against the typed builder: the direction of an edge's two ports is fixed by
//! the type of [`PipelineBuilder::connect`], each on its own.
//!
//! The one `compile_fail` example for `connect` joins an Audio output to a Text input, so it fails
//! on the modality whatever `connect` says about direction: a `connect` whose source is a port of
//! any direction passes every case of the crate. Here the direction of each parameter is read
//! back through type inference. A closure handed to [`direction_of`] takes a port of an unnamed
//! direction and passes it to `connect`; only `connect`'s own signature can name that direction,
//! so the case does not compile once either parameter stops naming one, and it fails if the
//! direction named is the wrong one.

use std::num::NonZeroUsize;

use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, NodeRole, OverloadPolicy,
    PipelineSpec, PortDirection, PortDirectionType, PortHandle, PortModality, PortTraits, modality,
};

/// The one profile the case declares, and no adapter.
struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

/// The direction `use_port` requires of the port it is handed, as its body's calls decide it.
fn direction_of<'b, D: PortDirectionType, M: PortModality>(
    use_port: impl FnOnce(&PortHandle<'b, D, M>) -> Option<()>,
) -> PortDirection {
    let _ = use_port;
    D::DIRECTION
}

fn traits() -> PortTraits {
    PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: false,
        end_marker_required: false,
        provisional_allowed: false,
    }
}

#[test]
fn connect_takes_an_output_port_as_source_and_an_input_port_as_sink() {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let spec = PipelineSpec {
        name: "direction".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::Batch,
        required_capabilities: Vec::new(),
    };
    let profile_id = profile.profile_id();
    let catalog = Profiles(profile);
    let mut declaration = Declaration::new();
    let (_, (source, sink)) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let node = pipeline.node("node", NodeRole::Transform, Vec::new());
            let out = pipeline.output::<modality::Text>(&node, "out", traits());
            let into = pipeline.input::<modality::Text>(&node, "in", traits());
            let source = direction_of(|from| {
                pipeline.connect(from, &into, NonZeroUsize::MIN, OverloadPolicy::Pause);
                None
            });
            let sink = direction_of(|to| {
                pipeline.connect(&out, to, NonZeroUsize::MIN, OverloadPolicy::Pause);
                None
            });
            (source, sink)
        })
        .expect("the profile is declared");

    assert_eq!(source, PortDirection::Output);
    assert_eq!(sink, PortDirection::Input);
    assert_eq!(
        declaration.edges().count(),
        0,
        "the closures are never called"
    );
}
