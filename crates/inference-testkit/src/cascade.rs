//! The voice cascade on the mocks: topology B of `docs/design/inference.md` (759-778) declared
//! with exposed ends, and the node implementations of one run of it.
//!
//! [`declare`] declares the cascade under the `offline_strict` profile with egress denied, with
//! the typed builder, and validates and plans it. Its nodes, in order:
//!
//! | node | role | implementation |
//! |---|---|---|
//! | `split` | fan_out | [`FanOut`]: hands every frame to the detector and the STT node |
//! | `vad` | policy | `inference-pipeline`'s voice activity detector, [`VAD`] |
//! | `turn policy` | policy | `inference-pipeline`'s turn policy, answered by a [`TurnStore`] |
//! | `stt` | model | [`SttNode`] over the [`MockRecogniser`] |
//! | `agent` | model | [`ModelNode`] over the [`MockGenerator`], given the STT node's turns |
//! | `text split` | fan_out | [`FanOut`]: hands the committed text to the TTS node and the egress |
//! | `tts` | model | [`TtsNode`] over the [`MockSynthesiser`] |
//!
//! It has no source and no sink node. It exposes four ends, each bounded and pausing on
//! overload: the ingress [`AUDIO_INGRESS`] at the input of `split`, in front of the detector, and
//! the egresses [`TRANSCRIPT_EGRESS`], every transcript event of the STT node,
//! [`TEXT_EGRESS`], the LLM node's output, its committed assistant text among it, and
//! [`SPEECH_EGRESS`], the frames and segment events of the TTS node. The detector consumes the
//! frames it hears and sends only speech activity on, so the frames reach the STT node through
//! the fan-out rather than through the detector. The LLM node takes the STT node's turns, so it
//! starts its generation at the completion of the first turn.
//!
//! The run's payload is [`CascadePayload`]: audio frames, `inference.stream` events, the speech
//! activity and turn events among them, and control events. [`VoiceCascade::nodes`] makes the
//! implementations of one run's nodes and the handles a test reads them through. The commit gate,
//! the acknowledging audio sink and the real adapters are not here: they belong to a local voice
//! cascade, whose adapters replace the mocks.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use inference_core::envelope::ControlKind;
use inference_core::id::{NodeId, RunId};
use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_llm::Generator;
use inference_pipeline::nodes::turn_policy::{
    CompleteTurnRefused, TurnCommands, TurnEvent, TurnPayload, TurnPolicy, TurnPolicyHandle,
};
use inference_pipeline::nodes::vad::{
    AudioPayload, SpeechActivity, VadConfig, VoiceActivityDetector,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, NodeImplementation, NodeRole,
    OverloadPolicy, PipelineId, PipelineSpec, PlanPipelineError, PlannedPipeline, PortId,
    PortTraits, ValidatePipelineError, modality,
};
use inference_stt::Recogniser;
use inference_stt::inference_audio::AudioFrame;
use inference_text::{OpenTurn, TurnCompleted, TurnId, TurnOpened};
use inference_tts::Synthesiser;

use crate::graph::RunLog;
use crate::llm::{Carried, MockGenerator, ModelHandle, ModelNode, ModelPayload};
use crate::session::{SessionEvent, SessionPayload, stream_session_event};
use crate::stream::{StreamDomain, StreamEvent, TurnMoveError};
use crate::stt::{FanOut, Heard, MockRecogniser, SttHandle, SttNode, SttPayload, SttPorts};
use crate::tts::{MockSynthesiser, Spoken, TtsHandle, TtsNode, TtsPayload};

/// The name of the ingress the utterance's frames enter by.
pub const AUDIO_INGRESS: &str = "audio";

/// The name of the egress carrying every transcript event of the STT node.
pub const TRANSCRIPT_EGRESS: &str = "transcript";

/// The name of the egress carrying the LLM node's output, its committed assistant text.
pub const TEXT_EGRESS: &str = "assistant_text";

/// The name of the egress carrying the TTS node's frames and segment events.
pub const SPEECH_EGRESS: &str = "speech";

/// The detector of the cascade: a frame is loud from a tenth of half scale, speech starts after
/// 30 ms of loud frames and ends after 200 ms of quiet ones.
pub const VAD: VadConfig = VadConfig {
    threshold: 0.05,
    attack: Duration::from_millis(30),
    hold: Duration::from_millis(200),
};

/// What the edges and exposures of the cascade carry.
#[derive(Debug, Clone, PartialEq)]
pub enum CascadePayload {
    /// A frame of audio; its envelope's `kind` is `inference.audio.AudioFrame`.
    Audio(AudioFrame),
    /// An `inference.stream` event; its envelope's `kind` is the event's declared name.
    Stream(StreamEvent),
    /// A control event; its envelope's `kind` is [`ControlKind::as_str`].
    Control(ControlKind),
}

impl From<ControlKind> for CascadePayload {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl AudioPayload for CascadePayload {
    fn into_audio(self) -> Result<AudioFrame, Self> {
        match self {
            Self::Audio(frame) => Ok(frame),
            other => Err(other),
        }
    }

    fn from_activity(activity: SpeechActivity) -> Self {
        Self::Stream(match activity {
            SpeechActivity::Started(started) => StreamEvent::SpeechStarted(started),
            SpeechActivity::Ended(ended) => StreamEvent::SpeechEnded(ended),
        })
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl TurnPayload for CascadePayload {
    fn into_activity(self) -> Result<SpeechActivity, Self> {
        match self {
            Self::Stream(StreamEvent::SpeechStarted(started)) => {
                Ok(SpeechActivity::Started(started))
            }
            Self::Stream(StreamEvent::SpeechEnded(ended)) => Ok(SpeechActivity::Ended(ended)),
            other => Err(other),
        }
    }

    fn from_turn(event: TurnEvent) -> Self {
        Self::Stream(match event {
            TurnEvent::Opened(opened) => StreamEvent::TurnOpened(opened),
            TurnEvent::Completed(completed) => StreamEvent::TurnCompleted(completed),
        })
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

/// A live session of `inference-facade` over the cascade writes its control events as
/// [`CascadePayload::Control`] and delivers a frame as `AudioChunk` and an `inference.stream`
/// event as the text pipeline's payload does, so one session event vocabulary covers both
/// topologies.
impl SessionPayload for CascadePayload {
    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }

    fn session_event(&self) -> Option<SessionEvent> {
        match self {
            Self::Audio(frame) => Some(SessionEvent::AudioChunk(frame.clone())),
            Self::Stream(event) => stream_session_event(event),
            Self::Control(_) => None,
        }
    }
}

impl SttPayload for CascadePayload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn heard(self) -> Heard {
        match self {
            Self::Audio(frame) => Heard::Audio(frame),
            Self::Stream(event) => Heard::Stream(event),
            Self::Control(kind) => Heard::Control(kind),
        }
    }
}

impl ModelPayload for CascadePayload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn carried(self) -> Carried {
        match self {
            Self::Stream(event) => Carried::Stream(event),
            Self::Control(kind) => Carried::Control(kind),
            Self::Audio(_) => Carried::Control(ControlKind::Error),
        }
    }
}

impl TtsPayload for CascadePayload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn audio(frame: AudioFrame) -> Self {
        Self::Audio(frame)
    }

    fn spoken(self) -> Spoken {
        match self {
            Self::Stream(event) => Spoken::Stream(event),
            Self::Control(kind) => Spoken::Control(kind),
            Self::Audio(_) => Spoken::Other,
        }
    }
}

/// The testkit's in-memory store of `inference.stream`, answering the turn policy's commands.
/// Clones share one store.
#[derive(Debug, Clone, Default)]
pub struct TurnStore(Arc<Mutex<StreamDomain>>);

impl TurnStore {
    /// Reads or changes the store.
    pub fn with<T>(&self, read: impl FnOnce(&mut StreamDomain) -> T) -> T {
        read(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl TurnCommands for TurnStore {
    fn open_turn(&mut self, input: OpenTurn) -> Result<TurnOpened, String> {
        Ok(self.with(|stream| stream.open_turn(input)))
    }

    fn complete_turn(&mut self, turn_id: TurnId) -> Result<TurnCompleted, CompleteTurnRefused> {
        self.with(|stream| stream.complete_turn(turn_id))
            .map_err(|refused| match refused {
                TurnMoveError::Unknown(error) => CompleteTurnRefused::Unknown(error),
                TurnMoveError::Conflict(error) => CompleteTurnRefused::Conflict(error),
            })
    }
}

/// The mocks the cascade's model nodes are bound to, and the LLM's reply to an input text.
#[derive(Debug, Clone)]
pub struct Mocks {
    pub recogniser: MockRecogniser,
    pub generator: MockGenerator,
    pub synthesiser: MockSynthesiser,
    pub reply: fn(&str) -> String,
}

/// The nodes of the cascade and the ports its run's implementations are given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CascadeIds {
    pub split: NodeId,
    pub vad: NodeId,
    pub policy: NodeId,
    pub stt: NodeId,
    pub llm: NodeId,
    pub text_split: NodeId,
    pub tts: NodeId,
    pub stt_ports: SttPorts,
    pub llm_turns: PortId,
}

impl CascadeIds {
    /// Every node, in the order of the module documentation's table.
    #[must_use]
    pub const fn nodes(&self) -> [NodeId; 7] {
        [
            self.split,
            self.vad,
            self.policy,
            self.stt,
            self.llm,
            self.text_split,
            self.tts,
        ]
    }
}

/// The cascade declared, validated and planned.
#[derive(Debug, Clone)]
pub struct VoiceCascade {
    pub pipeline_id: PipelineId,
    pub planned: PlannedPipeline,
    pub ids: CascadeIds,
}

/// Why [`declare`] answered no cascade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CascadeRefused {
    /// `inference.graph.ValidatePipeline` refused it.
    Invalid(ValidatePipelineError),
    /// Planning refused it.
    Unplannable(PlanPipelineError),
}

impl fmt::Display for CascadeRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => write!(f, "the cascade is invalid: {error:?}"),
            Self::Unplannable(error) => write!(f, "the cascade cannot be planned: {error:?}"),
        }
    }
}

impl std::error::Error for CascadeRefused {}

/// The profile the cascade is declared under and the adapters of its mocks.
struct Fixture {
    profile: DeploymentProfile,
    adapters: [Adapter; 3],
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.adapter_id() == adapter_id)
    }
}

/// Declares the cascade in `declaration`, its model nodes bound to the adapters of `mocks`, and
/// validates and plans it; see the module documentation.
///
/// # Errors
///
/// [`CascadeRefused`] when the declaration does not validate or plan, which the fixed cascade
/// does unless the mocks claim less than its model nodes require.
///
/// # Panics
///
/// Never: the fixture's catalog holds the profile and the three adapters the builder asks for.
pub fn declare(
    declaration: &mut Declaration,
    mocks: &Mocks,
) -> Result<VoiceCascade, CascadeRefused> {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture {
        profile,
        adapters: [
            mocks.recogniser.adapter().clone(),
            mocks.generator.adapter().clone(),
            mocks.synthesiser.adapter().clone(),
        ],
    };
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
    let spec = PipelineSpec {
        name: "voice cascade".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let capacity = NonZeroUsize::new(64).expect("64");
    let streaming = vec![Capability::IncrementalInput, Capability::OutputStreaming];
    let (pipeline_id, ids) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let split = pipeline.node("split", NodeRole::FanOut, Vec::new());
            let vad = pipeline.node("vad", NodeRole::Policy, Vec::new());
            let policy = pipeline.node("turn policy", NodeRole::Policy, Vec::new());
            let stt = pipeline.node("stt", NodeRole::Model, streaming.clone());
            let llm = pipeline.node("agent", NodeRole::Model, vec![Capability::OutputStreaming]);
            let text_split = pipeline.node("text split", NodeRole::FanOut, Vec::new());
            let tts = pipeline.node("tts", NodeRole::Model, streaming.clone());

            let microphone = pipeline.input::<modality::Audio>(&split, "audio", audio);
            let to_vad = pipeline.output::<modality::Audio>(&split, "vad", audio);
            let to_stt = pipeline.output::<modality::Audio>(&split, "stt", audio);
            let heard = pipeline.input::<modality::Audio>(&vad, "audio", audio);
            let detected = pipeline.output::<modality::Control>(&vad, "activity", control);
            let activity = pipeline.input::<modality::Control>(&policy, "activity", control);
            let turns = pipeline.output::<modality::Control>(&policy, "turns", control);
            let stt_audio = pipeline.input::<modality::Audio>(&stt, "audio", audio);
            let stt_turns = pipeline.input::<modality::Control>(&stt, "turns", control);
            let transcript = pipeline.output::<modality::Text>(&stt, "transcript", text);
            let committed = pipeline.output::<modality::Text>(&stt, "committed_text", text);
            let turns_out = pipeline.output::<modality::Control>(&stt, "turns_out", control);
            let user_input = pipeline.input::<modality::Text>(&llm, "user_input", text);
            let llm_turns = pipeline.input::<modality::Control>(&llm, "turns", control);
            let answer = pipeline.output::<modality::Text>(&llm, "committed_text", text);
            let answered = pipeline.input::<modality::Text>(&text_split, "text", text);
            let to_tts = pipeline.output::<modality::Text>(&text_split, "tts", text);
            let to_app = pipeline.output::<modality::Text>(&text_split, "assistant_text", text);
            let speak = pipeline.input::<modality::Text>(&tts, "text", text);
            let spoken = pipeline.output::<modality::Audio>(&tts, "audio", audio);

            let pause = OverloadPolicy::Pause;
            pipeline.connect(&to_vad, &heard, capacity, pause);
            pipeline.connect(&to_stt, &stt_audio, capacity, pause);
            pipeline.connect(&detected, &activity, capacity, pause);
            pipeline.connect(&turns, &stt_turns, capacity, pause);
            pipeline.connect(&committed, &user_input, capacity, pause);
            pipeline.connect(&turns_out, &llm_turns, capacity, pause);
            pipeline.connect(&answer, &answered, capacity, pause);
            pipeline.connect(&to_tts, &speak, capacity, pause);
            pipeline.expose(&microphone, AUDIO_INGRESS, capacity, pause);
            pipeline.expose(&transcript, TRANSCRIPT_EGRESS, capacity, pause);
            pipeline.expose(&to_app, TEXT_EGRESS, capacity, pause);
            pipeline.expose(&spoken, SPEECH_EGRESS, capacity, pause);
            for (node, adapter) in [&stt, &llm, &tts].into_iter().zip(&catalog.adapters) {
                pipeline
                    .bind(node, adapter.adapter_id())
                    .expect("the catalog holds the adapter");
            }
            CascadeIds {
                split: split.id(),
                vad: vad.id(),
                policy: policy.id(),
                stt: stt.id(),
                llm: llm.id(),
                text_split: text_split.id(),
                tts: tts.id(),
                stt_ports: SttPorts {
                    audio: stt_audio.id(),
                    turns: stt_turns.id(),
                    committed_text: committed.id(),
                    turns_out: turns_out.id(),
                    transcript: Some(transcript.id()),
                },
                llm_turns: llm_turns.id(),
            }
        })
        .expect("the catalog holds the profile");
    let validated = declaration
        .validate(pipeline_id)
        .map_err(CascadeRefused::Invalid)?;
    let planned = declaration
        .plan(validated, &catalog)
        .map_err(CascadeRefused::Unplannable)?;
    Ok(VoiceCascade {
        pipeline_id,
        planned,
        ids,
    })
}

/// The test's side of a run's nodes.
#[derive(Debug, Clone)]
pub struct CascadeHandles {
    pub policy: TurnPolicyHandle,
    pub stt: SttHandle,
    pub llm: ModelHandle,
    pub tts: TtsHandle,
}

/// The implementations of one run's nodes, by node, and their handles.
pub struct CascadeRun {
    pub implementations: HashMap<NodeId, Box<dyn NodeImplementation<CascadePayload>>>,
    pub handles: CascadeHandles,
}

impl fmt::Debug for CascadeRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CascadeRun")
            .field("nodes", &self.implementations.keys().collect::<Vec<_>>())
            .field("handles", &self.handles)
            .finish()
    }
}

impl VoiceCascade {
    /// The implementations of the nodes of the run `run_id` over `mocks`, the turn policy's
    /// commands answered by `turns` and what the nodes emit recorded in `log`.
    #[must_use]
    pub fn nodes(&self, run_id: RunId, mocks: Mocks, turns: TurnStore, log: RunLog) -> CascadeRun {
        let ids = self.ids;
        let policy = TurnPolicy::new(run_id, turns);
        let stt = SttNode::new(run_id, mocks.recogniser, ids.stt_ports, log.clone());
        let llm = ModelNode::carrying(run_id, mocks.generator, mocks.reply, log.clone())
            .with_turns(ids.llm_turns);
        let tts = TtsNode::new(run_id, mocks.synthesiser, log);
        let handles = CascadeHandles {
            policy: policy.handle(),
            stt: stt.handle(),
            llm: llm.handle(),
            tts: tts.handle(),
        };
        let implementations: HashMap<NodeId, Box<dyn NodeImplementation<CascadePayload>>> =
            HashMap::from([
                (
                    ids.split,
                    Box::new(FanOut::new()) as Box<dyn NodeImplementation<CascadePayload>>,
                ),
                (ids.vad, Box::new(VoiceActivityDetector::new(run_id, VAD))),
                (ids.policy, Box::new(policy)),
                (ids.stt, Box::new(stt)),
                (ids.llm, Box::new(llm)),
                (ids.text_split, Box::new(FanOut::new())),
                (ids.tts, Box::new(tts)),
            ]);
        CascadeRun {
            implementations,
            handles,
        }
    }
}
