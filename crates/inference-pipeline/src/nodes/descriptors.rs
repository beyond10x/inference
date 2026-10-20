//! The node descriptors of the node implementations of this crate, as
//! `inference.graph.DeclareNodeDescriptor` publishes them (`docs/design/inference.md` 473-482,
//! 491): the voice activity detector, the turn policy, the tool authority, the tool executor and
//! the commit gate. A declarative graph names a node by its kind ([`VAD`], [`TURN_POLICY`],
//! [`TOOL_AUTHORITY`], [`TOOL_EXECUTOR`], [`COMMIT_GATE`]) and is checked against these before
//! anything is declared.
//!
//! The ports carry the names and traits the node expects to be handed: the detector takes `audio`
//! frames incrementally and sends Control `activity`; the turn policy takes that `activity` and
//! sends Control `turns`; the authority takes Structured `proposed` calls and sends `authorized`
//! ones, and `rejected` ones when it is constructed to, so that output is optional; the executor
//! takes `authorized` calls and sends their `outcomes`; the gate takes Text on `input` and passes
//! it on `output`, and, placed between STT and LLM, takes Control `turns` and passes them on
//! `turns_out`, a lane that is optional. Every port is reliable and admits committed data only,
//! but the gate's `input`, which takes provisional data to hold it back, and every one needs an
//! end marker, since each node ends on the control event `stop`.

use inference_core::model::Modality;

use crate::{
    DeclareNodeDescriptor, Delivery, Essentiality, NodeKind, NodeRole, PortDescriptor,
    PortDirection, PortTraits,
};

/// The kind of the voice activity detector.
pub const VAD: &str = "vad";
/// The kind of the turn policy.
pub const TURN_POLICY: &str = "turn_policy";
/// The kind of the tool authority.
pub const TOOL_AUTHORITY: &str = "tool_authority";
/// The kind of the tool executor.
pub const TOOL_EXECUTOR: &str = "tool_executor";
/// The kind of the commit gate.
pub const COMMIT_GATE: &str = "commit_gate";

/// Essential, reliable whole frames with an end marker, committed data only.
const FRAMES: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: false,
    end_marker_required: true,
    provisional_allowed: false,
};

/// As [`FRAMES`], arriving incrementally.
const STREAMED: PortTraits = PortTraits {
    incremental: true,
    ..FRAMES
};

fn port(
    name: &str,
    direction: PortDirection,
    modality: Modality,
    traits: PortTraits,
) -> PortDescriptor {
    PortDescriptor::new(name, direction, modality, traits)
}

fn descriptor(kind: &str, role: NodeRole, ports: Vec<PortDescriptor>) -> DeclareNodeDescriptor {
    DeclareNodeDescriptor {
        kind: NodeKind::new(kind),
        role,
        requires: Vec::new(),
        ports,
    }
}

/// The voice activity detector ([`super::vad::VoiceActivityDetector`]): role `policy`, Audio
/// `audio` in, Control `activity` out.
#[must_use]
pub fn vad() -> DeclareNodeDescriptor {
    descriptor(
        VAD,
        NodeRole::Policy,
        vec![
            port("audio", PortDirection::Input, Modality::Audio, STREAMED),
            port("activity", PortDirection::Output, Modality::Control, FRAMES),
        ],
    )
}

/// The turn policy ([`super::turn_policy::TurnPolicy`]): role `policy`, Control `activity` in,
/// Control `turns` out.
#[must_use]
pub fn turn_policy() -> DeclareNodeDescriptor {
    descriptor(
        TURN_POLICY,
        NodeRole::Policy,
        vec![
            port("activity", PortDirection::Input, Modality::Control, FRAMES),
            port("turns", PortDirection::Output, Modality::Control, FRAMES),
        ],
    )
}

/// The tool authority ([`super::ToolAuthority`]): role `policy`, Structured `proposed` in,
/// `authorized` out and, optional, `rejected` out.
#[must_use]
pub fn tool_authority() -> DeclareNodeDescriptor {
    descriptor(
        TOOL_AUTHORITY,
        NodeRole::Policy,
        vec![
            port(
                "proposed",
                PortDirection::Input,
                Modality::Structured,
                FRAMES,
            ),
            port(
                "authorized",
                PortDirection::Output,
                Modality::Structured,
                FRAMES,
            ),
            port(
                "rejected",
                PortDirection::Output,
                Modality::Structured,
                PortTraits {
                    essentiality: Essentiality::Optional,
                    ..FRAMES
                },
            ),
        ],
    )
}

/// The tool executor ([`super::ToolExecutorNode`]): role `tool`, Structured `authorized` in,
/// `outcomes` out.
#[must_use]
pub fn tool_executor() -> DeclareNodeDescriptor {
    descriptor(
        TOOL_EXECUTOR,
        NodeRole::Tool,
        vec![
            port(
                "authorized",
                PortDirection::Input,
                Modality::Structured,
                FRAMES,
            ),
            port(
                "outcomes",
                PortDirection::Output,
                Modality::Structured,
                FRAMES,
            ),
        ],
    )
}

/// The commit gate ([`super::commit_gate::CommitGate`]): role `policy`, Text `input` in, taking
/// provisional data, and `output` out; Control `turns` in and `turns_out` out, both optional.
#[must_use]
pub fn commit_gate() -> DeclareNodeDescriptor {
    let optional = PortTraits {
        essentiality: Essentiality::Optional,
        ..FRAMES
    };
    descriptor(
        COMMIT_GATE,
        NodeRole::Policy,
        vec![
            port(
                "input",
                PortDirection::Input,
                Modality::Text,
                PortTraits {
                    provisional_allowed: true,
                    ..STREAMED
                },
            ),
            port("output", PortDirection::Output, Modality::Text, STREAMED),
            port("turns", PortDirection::Input, Modality::Control, optional),
            port(
                "turns_out",
                PortDirection::Output,
                Modality::Control,
                optional,
            ),
        ],
    )
}

/// The five descriptors of this crate, in the order the module lists them.
#[must_use]
pub fn all() -> Vec<DeclareNodeDescriptor> {
    vec![
        vad(),
        turn_policy(),
        tool_authority(),
        tool_executor(),
        commit_gate(),
    ]
}
