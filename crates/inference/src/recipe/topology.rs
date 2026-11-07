//! The topologies of the five built-in recipes, each declared into a pipeline builder with every
//! exposure at the recipe's capacity under `OverloadPolicy::Pause`. A role's node takes the
//! requirements `requirement` returns for its role and default.

use std::num::NonZeroUsize;

use inference_core::model::Capability;
use inference_pipeline::{Delivery, Essentiality, NodeRole, OverloadPolicy, PortTraits, modality};

use super::MAX_TOOL_ITERATIONS;
use crate::Role;
use crate::graph::Builder;

/// What a role's node requires: the caller's requirement for the role, or the default.
pub(super) type Requirement<'r> = dyn Fn(Role, &[Capability]) -> Vec<Capability> + 'r;

/// The capacity of the cascade's internal edges and its playback loops' bound.
const EDGE_CAPACITY: NonZeroUsize = NonZeroUsize::MIN.saturating_add(63);

/// The capacity of the tool loop's edges.
const TOOL_CAPACITY: NonZeroUsize = NonZeroUsize::MIN.saturating_add(3);

const STREAM: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

const PAUSE: OverloadPolicy = OverloadPolicy::Pause;

pub(super) fn chat(
    pipeline: &mut Builder<'_, '_>,
    capacity: NonZeroUsize,
    requirement: &Requirement<'_>,
) {
    let agent = pipeline.node(
        "agent",
        NodeRole::Model,
        requirement(Role::Llm, &[Capability::OutputStreaming]),
    );
    let input = pipeline.input::<modality::Text>(&agent, "user_input", STREAM);
    let output = pipeline.output::<modality::Text>(&agent, "committed_text", STREAM);
    pipeline.expose(&input, "user_input", capacity, PAUSE);
    pipeline.expose(&output, "assistant_text", capacity, PAUSE);
}

pub(super) fn transcribe(
    pipeline: &mut Builder<'_, '_>,
    capacity: NonZeroUsize,
    requirement: &Requirement<'_>,
) {
    let stt = pipeline.node("stt", NodeRole::Model, requirement(Role::Stt, &[]));
    let audio = pipeline.input::<modality::Audio>(&stt, "audio", STREAM);
    let committed = pipeline.output::<modality::Text>(&stt, "committed_text", STREAM);
    pipeline.expose(&audio, "audio", capacity, PAUSE);
    pipeline.expose(&committed, "transcript", capacity, PAUSE);
}

pub(super) fn speak(
    pipeline: &mut Builder<'_, '_>,
    capacity: NonZeroUsize,
    requirement: &Requirement<'_>,
) {
    let tts = pipeline.node("tts", NodeRole::Model, requirement(Role::Tts, &[]));
    let text = pipeline.input::<modality::Text>(&tts, "text", STREAM);
    let audio = pipeline.output::<modality::Audio>(&tts, "audio", STREAM);
    pipeline.expose(&text, "text", capacity, PAUSE);
    pipeline.expose(&audio, "audio", capacity, PAUSE);
}

/// The reference cascade's topology (`examples/local-voice-cascade/src/lib.rs`, `declare`).
pub(super) fn voice_cascade(
    pipeline: &mut Builder<'_, '_>,
    capacity: NonZeroUsize,
    requirement: &Requirement<'_>,
) {
    let audio = STREAM;
    let control = PortTraits {
        incremental: false,
        ..audio
    };
    let text = audio;
    let provisional = PortTraits {
        provisional_allowed: true,
        ..text
    };
    let edge = EDGE_CAPACITY;
    let split = pipeline.node("split", NodeRole::FanOut, Vec::new());
    let vad = pipeline.node("vad", NodeRole::Policy, Vec::new());
    let policy = pipeline.node("turn policy", NodeRole::Policy, Vec::new());
    let stt = pipeline.node("stt", NodeRole::Model, requirement(Role::Stt, &[]));
    let input_gate = pipeline.node("input gate", NodeRole::Policy, Vec::new());
    let turn_split = pipeline.node("turn split", NodeRole::FanOut, Vec::new());
    let llm = pipeline.node(
        "agent",
        NodeRole::Model,
        requirement(Role::Llm, &[Capability::OutputStreaming]),
    );
    let text_split = pipeline.node("text split", NodeRole::FanOut, Vec::new());
    let output_gate = pipeline.node("output gate", NodeRole::Policy, Vec::new());
    let tts = pipeline.node(
        "tts",
        NodeRole::Model,
        requirement(
            Role::Tts,
            &[Capability::IncrementalInput, Capability::OutputStreaming],
        ),
    );
    let sink = pipeline.node("speaker", NodeRole::Sink, Vec::new());

    let microphone = pipeline.input::<modality::Audio>(&split, "audio", audio);
    let to_vad = pipeline.output::<modality::Audio>(&split, "vad", audio);
    let to_stt = pipeline.output::<modality::Audio>(&split, "stt", audio);
    let heard = pipeline.input::<modality::Audio>(&vad, "audio", audio);
    let detected = pipeline.output::<modality::Control>(&vad, "activity", control);
    let activity = pipeline.input::<modality::Control>(&policy, "activity", control);
    let turns = pipeline.output::<modality::Control>(&policy, "turns", control);
    let stt_audio = pipeline.input::<modality::Audio>(&stt, "audio", audio);
    let stt_turns = pipeline.input::<modality::Control>(&stt, "turns", control);
    let transcript = pipeline.output::<modality::Text>(&stt, "transcript", provisional);
    let committed = pipeline.output::<modality::Text>(&stt, "committed_text", text);
    let turns_out = pipeline.output::<modality::Control>(&stt, "turns_out", control);
    let gate_in = pipeline.input::<modality::Text>(&input_gate, "input", provisional);
    let gate_out = pipeline.output::<modality::Text>(&input_gate, "output", text);
    let gate_turns = pipeline.input::<modality::Control>(&input_gate, "turns", control);
    let gate_turns_out = pipeline.output::<modality::Control>(&input_gate, "turns_out", control);
    let user_input = pipeline.input::<modality::Text>(&llm, "user_input", text);
    let llm_turns = pipeline.input::<modality::Control>(&llm, "turns", control);
    let answer = pipeline.output::<modality::Text>(&llm, "committed_text", text);
    let answered = pipeline.input::<modality::Text>(&text_split, "text", text);
    let to_gate = pipeline.output::<modality::Text>(&text_split, "gate", text);
    let to_app = pipeline.output::<modality::Text>(&text_split, "assistant_text", text);
    let held = pipeline.input::<modality::Text>(&output_gate, "input", provisional);
    let passed = pipeline.output::<modality::Text>(&output_gate, "output", text);
    let speak = pipeline.input::<modality::Text>(&tts, "text", text);
    let played_back = pipeline.input::<modality::Control>(&tts, "played", control);
    let spoken = pipeline.output::<modality::Audio>(&tts, "audio", audio);
    let playing = pipeline.input::<modality::Audio>(&sink, "audio", audio);
    let receipts = pipeline.output::<modality::Control>(&sink, "played", control);
    let split_turns = pipeline.input::<modality::Control>(&turn_split, "turns", control);
    let turns_to_llm = pipeline.output::<modality::Control>(&turn_split, "llm", control);
    let turns_to_sink = pipeline.output::<modality::Control>(&turn_split, "speaker", control);
    let sink_turns = pipeline.input::<modality::Control>(&sink, "turns", control);
    let tts_playback = pipeline.output::<modality::Control>(&tts, "playback", control);
    let llm_played = pipeline.input::<modality::Control>(&llm, "played", control);

    pipeline.connect(&to_vad, &heard, edge, PAUSE);
    pipeline.connect(&to_stt, &stt_audio, edge, PAUSE);
    pipeline.connect(&detected, &activity, edge, PAUSE);
    pipeline.connect(&turns, &stt_turns, edge, PAUSE);
    pipeline.connect(&transcript, &gate_in, edge, PAUSE);
    pipeline.connect(&turns_out, &gate_turns, edge, PAUSE);
    pipeline.connect(&gate_out, &user_input, edge, PAUSE);
    pipeline.connect(&gate_turns_out, &split_turns, edge, PAUSE);
    pipeline.connect(&turns_to_llm, &llm_turns, edge, PAUSE);
    pipeline.connect(&turns_to_sink, &sink_turns, edge, PAUSE);
    pipeline.connect(&answer, &answered, edge, PAUSE);
    pipeline.connect(&to_gate, &held, edge, PAUSE);
    pipeline.connect(&passed, &speak, edge, PAUSE);
    pipeline.connect(&spoken, &playing, edge, PAUSE);
    pipeline.connect_feedback(&receipts, &played_back, edge, PAUSE, edge);
    pipeline.connect_feedback(&tts_playback, &llm_played, edge, PAUSE, edge);
    pipeline.expose(&microphone, "audio", capacity, PAUSE);
    pipeline.expose(&committed, "transcript", capacity, PAUSE);
    pipeline.expose(&to_app, "assistant_text", capacity, PAUSE);
}

/// The native speech model's node and the runtime's tool nodes, wired as
/// `crates/inference-runtime/tests/tool_feedback_loop.rs` wires them around a model.
pub(super) fn native_voice(
    pipeline: &mut Builder<'_, '_>,
    capacity: NonZeroUsize,
    requirement: &Requirement<'_>,
) {
    let control = PortTraits {
        essentiality: Essentiality::ControlOnly,
        incremental: false,
        ..STREAM
    };
    let structured = PortTraits {
        essentiality: Essentiality::Optional,
        incremental: false,
        ..STREAM
    };
    let fail = OverloadPolicy::Fail;
    let speech = pipeline.node(
        "speech",
        NodeRole::Model,
        requirement(Role::Speech, &[Capability::NativeDuplex]),
    );
    let authority = pipeline.node("authority", NodeRole::Policy, Vec::new());
    let executor = pipeline.node("executor", NodeRole::Tool, Vec::new());
    let audio = pipeline.input::<modality::Audio>(&speech, "audio", STREAM);
    let control_in = pipeline.input::<modality::Control>(&speech, "control", control);
    let events = pipeline.output::<modality::Structured>(&speech, "events", STREAM);
    let tool_call = pipeline.output::<modality::Structured>(&speech, "tool_call", structured);
    let tool_result = pipeline.input::<modality::Structured>(&speech, "tool_result", structured);
    let proposed = pipeline.input::<modality::Structured>(&authority, "proposed", structured);
    let authorized = pipeline.output::<modality::Structured>(&authority, "authorized", structured);
    let rejected = pipeline.output::<modality::Structured>(&authority, "rejected", structured);
    let calls = pipeline.input::<modality::Structured>(&executor, "authorized", structured);
    let outcomes = pipeline.output::<modality::Structured>(&executor, "outcomes", structured);
    pipeline.connect_feedback(
        &tool_call,
        &proposed,
        TOOL_CAPACITY,
        fail,
        MAX_TOOL_ITERATIONS,
    );
    pipeline.connect(&authorized, &calls, TOOL_CAPACITY, fail);
    pipeline.connect(&outcomes, &tool_result, TOOL_CAPACITY, fail);
    pipeline.connect(&rejected, &tool_result, TOOL_CAPACITY, fail);
    pipeline.expose(&audio, "audio", capacity, PAUSE);
    pipeline.expose(&control_in, "control", capacity, PAUSE);
    pipeline.expose(&events, "events", capacity, PAUSE);
}
