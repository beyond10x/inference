//! The reference cascade's declaration, copied from `local_voice_cascade::declare_requiring`
//! (`examples/local-voice-cascade/src/lib.rs`) as it stands with the shared payload, so that the
//! comparison with `Recipe::voice_cascade()` survives the example's port onto the recipe.

use std::num::NonZeroUsize;

use inference::graph::{
    AdapterId, Capability, Catalog, Declaration, Delivery, Essentiality, InteractionMode, NodeRole,
    OverloadPolicy, PipelineId, PipelineSpec, PortTraits, ProfileId, modality,
};

/// Declares the reference cascade in `declaration` under `profile_id`, its nodes `stt`, `agent`
/// and `tts` bound to `adapters` in that order, the `stt` node requiring nothing.
pub fn declare(
    declaration: &mut Declaration,
    catalog: &dyn Catalog,
    profile_id: ProfileId,
    adapters: [AdapterId; 3],
) -> PipelineId {
    let audio = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let control = PortTraits {
        incremental: false,
        ..audio
    };
    let text = audio;
    let provisional = PortTraits {
        provisional_allowed: true,
        ..text
    };
    let spec = PipelineSpec {
        name: "local voice cascade".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let capacity = NonZeroUsize::new(64).expect("64");
    let pause = OverloadPolicy::Pause;
    let streaming = vec![Capability::IncrementalInput, Capability::OutputStreaming];
    let (pipeline_id, ()) = declaration
        .build(catalog, profile_id, spec, |pipeline| {
            let split = pipeline.node("split", NodeRole::FanOut, Vec::new());
            let vad = pipeline.node("vad", NodeRole::Policy, Vec::new());
            let policy = pipeline.node("turn policy", NodeRole::Policy, Vec::new());
            let stt = pipeline.node("stt", NodeRole::Model, Vec::new());
            let input_gate = pipeline.node("input gate", NodeRole::Policy, Vec::new());
            let turn_split = pipeline.node("turn split", NodeRole::FanOut, Vec::new());
            let llm = pipeline.node("agent", NodeRole::Model, vec![Capability::OutputStreaming]);
            let text_split = pipeline.node("text split", NodeRole::FanOut, Vec::new());
            let output_gate = pipeline.node("output gate", NodeRole::Policy, Vec::new());
            let tts = pipeline.node("tts", NodeRole::Model, streaming);
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
            let gate_turns_out =
                pipeline.output::<modality::Control>(&input_gate, "turns_out", control);
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
            let turns_to_sink =
                pipeline.output::<modality::Control>(&turn_split, "speaker", control);
            let sink_turns = pipeline.input::<modality::Control>(&sink, "turns", control);
            let tts_playback = pipeline.output::<modality::Control>(&tts, "playback", control);
            let llm_played = pipeline.input::<modality::Control>(&llm, "played", control);

            pipeline.connect(&to_vad, &heard, capacity, pause);
            pipeline.connect(&to_stt, &stt_audio, capacity, pause);
            pipeline.connect(&detected, &activity, capacity, pause);
            pipeline.connect(&turns, &stt_turns, capacity, pause);
            pipeline.connect(&transcript, &gate_in, capacity, pause);
            pipeline.connect(&turns_out, &gate_turns, capacity, pause);
            pipeline.connect(&gate_out, &user_input, capacity, pause);
            pipeline.connect(&gate_turns_out, &split_turns, capacity, pause);
            pipeline.connect(&turns_to_llm, &llm_turns, capacity, pause);
            pipeline.connect(&turns_to_sink, &sink_turns, capacity, pause);
            pipeline.connect(&answer, &answered, capacity, pause);
            pipeline.connect(&to_gate, &held, capacity, pause);
            pipeline.connect(&passed, &speak, capacity, pause);
            pipeline.connect(&spoken, &playing, capacity, pause);
            pipeline.connect_feedback(&receipts, &played_back, capacity, pause, capacity);
            pipeline.connect_feedback(&tts_playback, &llm_played, capacity, pause, capacity);
            pipeline.expose(&microphone, "audio", capacity, pause);
            pipeline.expose(&committed, "transcript", capacity, pause);
            pipeline.expose(&to_app, "assistant_text", capacity, pause);
            for (node, adapter) in [&stt, &llm, &tts].into_iter().zip(adapters) {
                pipeline
                    .bind(node, adapter)
                    .expect("the catalog holds the adapter");
            }
        })
        .expect("the catalog holds the profile");
    pipeline_id
}
