//! `inference::graph`: the pipeline's typed builder with [`Payload`](crate::Payload) fixed
//! (`docs/design/sdk.md` §3.4).
//!
//! A graph is declared with `inference-pipeline`'s typed builder, [`Builder`], into a
//! [`Declaration`] over a [`Catalog`] of a deployment profile and the adapters the model nodes
//! bind, then validated and planned. Its run and its session carry [`Payload`], so a node's
//! implementation is an [`Implementation`] and an envelope an [`Envelope`]. A port declared with
//! [`Builder::input_port`] or [`Builder::output_port`] takes its modality's default traits
//! ([`PortTraits::of`]), so a port names only its node, its name and its modality; a port that
//! admits provisional text says so with [`PortTraits::provisional`].
//!
//! The reference cascade, `examples/local-voice-cascade`, declared, validated and planned:
//!
//! ```
//! use std::num::NonZeroUsize;
//!
//! use inference::Payload;
//! use inference::graph::{
//!     Adapter, AdapterDescriptor, AdapterId, Capability, Catalog, ControlKind, DeclareAdapter,
//!     DeclareProfile, Declaration, DeploymentProfile, Egress, ExecutionForm, InteractionMode,
//!     Locality, Modality, ModelId, NodeRole, OverloadPolicy, PipelineSpec, PortTraits, ProfileId,
//!     ProfileKind, modality,
//! };
//!
//! struct Fixture {
//!     profile: DeploymentProfile,
//!     adapters: Vec<Adapter>,
//! }
//!
//! impl Catalog for Fixture {
//!     fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
//!         (self.profile.profile_id() == profile_id).then_some(&self.profile)
//!     }
//!
//!     fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
//!         self.adapters.iter().find(|adapter| adapter.adapter_id() == adapter_id)
//!     }
//! }
//!
//! let adapter = |capabilities: Vec<Capability>| {
//!     Adapter::declare(DeclareAdapter {
//!         model_id: ModelId::generate(),
//!         backend: "mock".to_owned(),
//!         contract_revision: "1".to_owned(),
//!         execution_form: ExecutionForm::InProcess,
//!         locality: Locality::SameProcess,
//!         capabilities,
//!         max_concurrency: 1,
//!         descriptor: AdapterDescriptor::default(),
//!     })
//!     .map(|(adapter, _)| adapter)
//! };
//! let streaming = vec![Capability::IncrementalInput, Capability::OutputStreaming];
//! let (profile, _) = DeploymentProfile::declare(DeclareProfile {
//!     kind: ProfileKind::OfflineStrict,
//!     egress: Egress::Denied,
//!     admitted: vec![Locality::SameProcess],
//! })?;
//! let catalog = Fixture {
//!     profile,
//!     adapters: vec![
//!         adapter(Vec::new())?,
//!         adapter(vec![Capability::OutputStreaming])?,
//!         adapter(streaming.clone())?,
//!     ],
//! };
//! let spec = PipelineSpec {
//!     name: "local voice cascade".to_owned(),
//!     schema_revision: "1".to_owned(),
//!     mode: InteractionMode::TurnBased,
//!     required_capabilities: Vec::new(),
//! };
//! let capacity = NonZeroUsize::new(64).ok_or("64")?;
//! let pause = OverloadPolicy::Pause;
//! let provisional = PortTraits::of(Modality::Text).provisional();
//!
//! let mut declaration = Declaration::new();
//! let (pipeline_id, bound) = declaration.build(&catalog, catalog.profile.profile_id(), spec, |g| {
//!     let split = g.node("split", NodeRole::FanOut, Vec::new());
//!     let vad = g.node("vad", NodeRole::Policy, Vec::new());
//!     let policy = g.node("turn policy", NodeRole::Policy, Vec::new());
//!     let stt = g.node("stt", NodeRole::Model, Vec::new());
//!     let input_gate = g.node("input gate", NodeRole::Policy, Vec::new());
//!     let turn_split = g.node("turn split", NodeRole::FanOut, Vec::new());
//!     let llm = g.node("agent", NodeRole::Model, vec![Capability::OutputStreaming]);
//!     let text_split = g.node("text split", NodeRole::FanOut, Vec::new());
//!     let output_gate = g.node("output gate", NodeRole::Policy, Vec::new());
//!     let tts = g.node("tts", NodeRole::Model, streaming);
//!     let sink = g.node("speaker", NodeRole::Sink, Vec::new());
//!
//!     let microphone = g.input_port::<modality::Audio>(&split, "audio");
//!     let to_vad = g.output_port::<modality::Audio>(&split, "vad");
//!     let to_stt = g.output_port::<modality::Audio>(&split, "stt");
//!     let heard = g.input_port::<modality::Audio>(&vad, "audio");
//!     let detected = g.output_port::<modality::Control>(&vad, "activity");
//!     let activity = g.input_port::<modality::Control>(&policy, "activity");
//!     let turns = g.output_port::<modality::Control>(&policy, "turns");
//!     let stt_audio = g.input_port::<modality::Audio>(&stt, "audio");
//!     let stt_turns = g.input_port::<modality::Control>(&stt, "turns");
//!     let transcript = g.output::<modality::Text>(&stt, "transcript", provisional);
//!     let committed = g.output_port::<modality::Text>(&stt, "committed_text");
//!     let turns_out = g.output_port::<modality::Control>(&stt, "turns_out");
//!     let gate_in = g.input::<modality::Text>(&input_gate, "input", provisional);
//!     let gate_out = g.output_port::<modality::Text>(&input_gate, "output");
//!     let gate_turns = g.input_port::<modality::Control>(&input_gate, "turns");
//!     let gate_turns_out = g.output_port::<modality::Control>(&input_gate, "turns_out");
//!     let user_input = g.input_port::<modality::Text>(&llm, "user_input");
//!     let llm_turns = g.input_port::<modality::Control>(&llm, "turns");
//!     let answer = g.output_port::<modality::Text>(&llm, "committed_text");
//!     let answered = g.input_port::<modality::Text>(&text_split, "text");
//!     let to_gate = g.output_port::<modality::Text>(&text_split, "gate");
//!     let to_app = g.output_port::<modality::Text>(&text_split, "assistant_text");
//!     let held = g.input::<modality::Text>(&output_gate, "input", provisional);
//!     let passed = g.output_port::<modality::Text>(&output_gate, "output");
//!     let speak = g.input_port::<modality::Text>(&tts, "text");
//!     let played_back = g.input_port::<modality::Control>(&tts, "played");
//!     let spoken = g.output_port::<modality::Audio>(&tts, "audio");
//!     let playing = g.input_port::<modality::Audio>(&sink, "audio");
//!     let receipts = g.output_port::<modality::Control>(&sink, "played");
//!     let split_turns = g.input_port::<modality::Control>(&turn_split, "turns");
//!     let turns_to_llm = g.output_port::<modality::Control>(&turn_split, "llm");
//!     let turns_to_sink = g.output_port::<modality::Control>(&turn_split, "speaker");
//!     let sink_turns = g.input_port::<modality::Control>(&sink, "turns");
//!     let tts_playback = g.output_port::<modality::Control>(&tts, "playback");
//!     let llm_played = g.input_port::<modality::Control>(&llm, "played");
//!
//!     g.connect(&to_vad, &heard, capacity, pause);
//!     g.connect(&to_stt, &stt_audio, capacity, pause);
//!     g.connect(&detected, &activity, capacity, pause);
//!     g.connect(&turns, &stt_turns, capacity, pause);
//!     g.connect(&transcript, &gate_in, capacity, pause);
//!     g.connect(&turns_out, &gate_turns, capacity, pause);
//!     g.connect(&gate_out, &user_input, capacity, pause);
//!     g.connect(&gate_turns_out, &split_turns, capacity, pause);
//!     g.connect(&turns_to_llm, &llm_turns, capacity, pause);
//!     g.connect(&turns_to_sink, &sink_turns, capacity, pause);
//!     g.connect(&answer, &answered, capacity, pause);
//!     g.connect(&to_gate, &held, capacity, pause);
//!     g.connect(&passed, &speak, capacity, pause);
//!     g.connect(&spoken, &playing, capacity, pause);
//!     g.connect_feedback(&receipts, &played_back, capacity, pause, capacity);
//!     g.connect_feedback(&tts_playback, &llm_played, capacity, pause, capacity);
//!     g.expose(&microphone, "audio", capacity, pause);
//!     g.expose(&committed, "transcript", capacity, pause);
//!     g.expose(&to_app, "assistant_text", capacity, pause);
//!     [&stt, &llm, &tts]
//!         .into_iter()
//!         .zip(&catalog.adapters)
//!         .map(|(node, adapter)| g.bind(node, adapter.adapter_id()))
//!         .collect::<Result<Vec<_>, _>>()
//! })?;
//! bound?;
//! let validated = declaration.validate(pipeline_id)?;
//! let planned = declaration.plan(validated, &catalog)?;
//! assert_eq!(planned.pipeline_id(), pipeline_id);
//! assert_eq!(declaration.nodes().count(), 11);
//!
//! // What the run's edges and the session carry: a control event among the rest.
//! assert!(matches!(Payload::from(ControlKind::Commit), Payload::Control(ControlKind::Commit)));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use inference_core::envelope::EventEnvelope;
use inference_pipeline::{NodeImplementation, PipelineBuilder};

pub use crate::payload::{Handoff, Payload};
pub use inference_core::envelope::ControlKind;
pub use inference_core::model::{
    Adapter, AdapterDescriptor, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality,
    Modality, ModelId,
};
pub use inference_core::policy::{
    DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind,
};
pub use inference_pipeline::{
    Catalog, Declaration, Delivery, EdgeId, Essentiality, Exposed, ExposureId, FeedbackLoopId,
    InteractionMode, Node, NodeError, NodeHandle, NodeId, NodeRole, OverloadPolicy, PipelineId,
    PipelineSpec, PlanPipelineError, PlannedPipeline, PortDirectionType, PortHandle, PortId,
    PortModality, PortTraits, USAGE_PORT, ValidatePipelineError, ValidatedPipeline, direction,
    modality,
};

/// The typed builder [`Declaration::build`] hands its closure.
pub type Builder<'d, 'b> = PipelineBuilder<'d, 'b>;

/// The implementation of a node of a run over [`Payload`].
pub type Implementation = Box<dyn NodeImplementation<Payload>>;

/// An envelope of a run over [`Payload`].
pub type Envelope = EventEnvelope<Payload>;
