//! The local voice cascade: topology B of `docs/design/inference.md` (759-778), STT, LLM and TTS
//! connected through typed pipeline nodes, the transcripts and the response segments explicitly
//! committed (demonstrator step 4, 1174), and played on a speaker that alone confirms playback
//! (388).
//!
//! [`declare`] declares the cascade with the typed builder under the `offline_strict` profile with
//! egress denied, admitting in-process adapters, and validates and plans it. Its nodes, in order:
//!
//! | node | role | implementation |
//! |---|---|---|
//! | `split` | fan_out | the testkit's `FanOut`: every frame to the detector and the STT node |
//! | `vad` | policy | `inference-pipeline`'s voice activity detector, [`VAD`] |
//! | `turn policy` | policy | `inference-pipeline`'s turn policy, answered by a [`TurnStore`] |
//! | `stt` | model | the testkit's `SttNode` over its mock recogniser |
//! | `input gate` | policy | a [`CommitGate`] of two lanes: the transcript, and the turn events |
//! | `turn split` | fan_out | `FanOut`: the gate's turn events to the LLM and the speaker |
//! | `agent` | model | the testkit's [`AgentNode`] over its mock generator: one generation per turn |
//! | `text split` | fan_out | `FanOut`: the LLM's output to the output gate and the egress |
//! | `output gate` | policy | a [`CommitGate`] of one lane: the LLM's output |
//! | `tts` | model | the testkit's [`SpeechNode`] over its mock synthesiser |
//! | `speaker` | sink | the device adapter's [`AcknowledgingSink`] on the audio host's speaker |
//!
//! The input gate passes the committed spans and the turn events to the LLM and holds back the
//! hypotheses; it refuses a revision of a span it passed as Committed. The output gate passes a
//! segment to the TTS node only with its commit, so the TTS node never queues a Tentative one. The
//! TTS node sends the synthesis's own frames to the sink, and the sink sends the receipt of each
//! segment it played back to the TTS node over a feedback edge, the only way a segment reaches
//! Played. The TTS node sends the `AudioPlayed` of each segment on to the LLM over a second
//! feedback edge, so that the LLM knows what of its answer was played.
//!
//! A turn that opens while the answer of an earlier one is not all Played is a barge-in
//! (632-641). The turn events reach the speaker as well as the LLM. The sink stops the speaker at
//! the `TurnOpened`, clearing its queue and reading the played position, and from then on drops
//! every frame of a generation that began before it, making no receipt of it: one it took frames
//! of, or one whose `GenerationStarted`, which the TTS node passes on ahead of the generation's
//! frames, names the epoch of an earlier turn, even before its first frame arrives. The LLM
//! supersedes its current generation at the same `TurnOpened` and sends its
//! `GenerationSuperseded` on; the output gate drops what it holds of that generation, and the TTS
//! node discards each of its segments not yet Played and passes the `GenerationSuperseded` on to
//! the sink, which takes it the same way and records the interruption then if no audio was left
//! to stop at the opening. A turn that opens once the answer was all Played interrupts nothing:
//! the LLM completes that generation. No node instance changes state.
//!
//! The cascade has no source node: the application feeds the ingress [`AUDIO_INGRESS`] at the
//! input of `split`, and reads the egresses [`TRANSCRIPT_EGRESS`], the committed spans of the STT
//! node, and [`TEXT_EGRESS`], the LLM's output, its committed assistant text among it. Every edge
//! and exposure is bounded and pauses on overload.
//!
//! The run's payload is [`VoicePayload`]. A frame of a segment and the receipt of its playback
//! are not `Clone`, and the payload is, since a fan-out copies what it hands on: each travels in a
//! [`Handoff`], which its first reader takes and every copy then holds empty, so the frames a
//! receipt is made of exist once. The speech edge and the receipt edge carry no fan-out.
//!
//! [`LocalCascade::nodes`] makes the implementations of one run's nodes, generic over the audio
//! host the speaker is opened on, and the handles a caller reads them through. Through them the
//! wiring records, per turn, the time from the input gate passing the turn's completion to the
//! sink handing the first frame after it to the speaker, the design's "committed input to first
//! playable audio" (1022): [`CascadeHandles::committed_input_to_first_audio`]; and, per
//! interruption, the time from the sink taking the `TurnOpened` to the speaker's stop returning,
//! the design's "interruption to playback stop" (1023):
//! [`CascadeHandles::interruption_to_playback_stop`]. Each is a value recorded, not a target
//! asserted. [`LocalCascade::nodes_with`] makes the same nodes over another synthesiser made of
//! the mock, such as one scripted to emit late.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use inference_audio::AudioFrame;
use inference_audio_device::acknowledging::{
    AcknowledgingHandle, AcknowledgingSink, Playable, PlaybackPayload,
};
use inference_audio_device::{AudioHost, DeviceError, DeviceSelector, PlayoutConfig, Speaker};
use inference_core::envelope::ControlKind;
use inference_core::id::{NodeId, RunId};
use inference_core::model::{Adapter, AdapterId, Capability, Locality};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::commit_gate::{
    CommitGate, CommitGateHandle, GateLane, GatePayload, Gated,
};
use inference_pipeline::nodes::turn_policy::{
    TurnEvent, TurnPayload, TurnPolicy, TurnPolicyHandle,
};
use inference_pipeline::nodes::vad::{
    AudioPayload, SpeechActivity, VadConfig, VoiceActivityDetector,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, NodeImplementation, NodeRole,
    OverloadPolicy, PipelineId, PipelineSpec, PlannedPipeline, PortId, PortTraits, modality,
};
use inference_testkit::cascade::agent::{AgentHandle, AgentNode};
use inference_testkit::cascade::speech::{
    Said, SpeechHandle, SpeechNode, SpeechPayload, SpeechPorts,
};
use inference_testkit::cascade::{CascadeRefused, Mocks, TurnStore};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{Carried, ModelPayload};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::inference_stt::Recogniser;
use inference_testkit::stt::{FanOut, Heard, SttHandle, SttNode, SttPayload, SttPorts};
use inference_testkit::tts::MockSynthesiser;
use inference_text::{SegmentId, TurnId};
use inference_tts::{PlaybackReceipt, SegmentAudio, Synthesiser};

/// The name of the ingress the utterance's frames enter by.
pub const AUDIO_INGRESS: &str = "audio";

/// The name of the egress carrying the committed spans of the STT node.
pub const TRANSCRIPT_EGRESS: &str = "transcript";

/// The name of the egress carrying the LLM node's output, its committed assistant text.
pub const TEXT_EGRESS: &str = "assistant_text";

/// The detector of the cascade: a frame is loud from a tenth of half scale, speech starts after
/// 30 ms of loud frames and ends after 200 ms of quiet ones.
pub const VAD: VadConfig = VadConfig {
    threshold: 0.05,
    attack: Duration::from_millis(30),
    hold: Duration::from_millis(200),
};

/// A value handed along one edge and taken once: the first reader takes it, and every copy of
/// the handoff, a fan-out's among them, then holds nothing.
pub struct Handoff<T>(Arc<Mutex<Option<T>>>);

impl<T> Handoff<T> {
    /// A handoff holding `value`.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }

    /// The value, if no reader took it before.
    #[must_use]
    pub fn take(&self) -> Option<T> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    /// Whether a reader took the value.
    #[must_use]
    pub fn taken(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
    }
}

impl<T> Clone for Handoff<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> fmt::Debug for Handoff<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handoff")
            .field("taken", &self.taken())
            .finish()
    }
}

/// What the edges and exposures of the cascade carry.
#[derive(Debug, Clone)]
pub enum VoicePayload {
    /// A frame of audio; its envelope's `kind` is `inference.audio.AudioFrame`.
    Audio(AudioFrame),
    /// An `inference.stream` event; its envelope's `kind` is the event's declared name.
    Stream(StreamEvent),
    /// A frame of a segment as the synthesis emitted it, taken once by the sink.
    Speech(Handoff<SegmentAudio>),
    /// The sink's receipt of the playback of a segment, taken once by the TTS node.
    Receipt(Handoff<(SegmentId, PlaybackReceipt)>),
    /// A control event; its envelope's `kind` is [`ControlKind::as_str`].
    Control(ControlKind),
}

impl From<ControlKind> for VoicePayload {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl AudioPayload for VoicePayload {
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

impl TurnPayload for VoicePayload {
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

impl SttPayload for VoicePayload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn heard(self) -> Heard {
        match self {
            Self::Audio(frame) => Heard::Audio(frame),
            Self::Stream(event) => Heard::Stream(event),
            Self::Control(kind) => Heard::Control(kind),
            Self::Speech(_) | Self::Receipt(_) => Heard::Other,
        }
    }
}

impl ModelPayload for VoicePayload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn carried(self) -> Carried {
        match self {
            Self::Stream(event) => Carried::Stream(event),
            Self::Control(kind) => Carried::Control(kind),
            Self::Audio(_) | Self::Speech(_) | Self::Receipt(_) => {
                Carried::Control(ControlKind::Error)
            }
        }
    }
}

impl GatePayload for VoicePayload {
    fn gated(&self) -> Gated {
        let Self::Stream(event) = self else {
            return Gated::Other;
        };
        match event {
            StreamEvent::TranscriptHypothesis(event) => Gated::Hypothesis {
                span_id: event.span_id,
            },
            StreamEvent::TranscriptRevision(event) => Gated::Revision {
                span_id: event.span_id,
            },
            StreamEvent::TranscriptCommitted(event) => Gated::SpanCommitted {
                span_id: event.span_id,
            },
            StreamEvent::TurnOpened(opened) => Gated::Turn(TurnEvent::Opened(*opened)),
            StreamEvent::TurnCompleted(completed) => Gated::Turn(TurnEvent::Completed(*completed)),
            StreamEvent::TextDelta(delta) => Gated::Delta {
                segment_id: delta.segment_id,
                generation_id: delta.generation_id,
            },
            StreamEvent::TextSegmentCommitted(committed) => Gated::SegmentCommitted {
                segment_id: committed.segment_id,
            },
            StreamEvent::GenerationCompleted(ended) => Gated::GenerationEnded {
                generation_id: ended.generation_id,
            },
            StreamEvent::GenerationCancelled(ended) => Gated::GenerationEnded {
                generation_id: ended.generation_id,
            },
            StreamEvent::GenerationSuperseded(ended) => Gated::GenerationEnded {
                generation_id: ended.generation_id,
            },
            _ => Gated::Other,
        }
    }
}

impl SpeechPayload for VoicePayload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn speech(audio: SegmentAudio) -> Self {
        Self::Speech(Handoff::new(audio))
    }

    fn said(self) -> Said {
        match self {
            Self::Stream(event) => Said::Stream(event),
            Self::Receipt(receipt) => match receipt.take() {
                Some((segment_id, receipt)) => Said::Receipt {
                    segment_id,
                    receipt,
                },
                None => Said::Other,
            },
            Self::Control(kind) => Said::Control(kind),
            Self::Audio(_) | Self::Speech(_) => Said::Other,
        }
    }
}

impl PlaybackPayload for VoicePayload {
    fn playable(self) -> Playable {
        match self {
            Self::Speech(audio) => audio.take().map_or(Playable::Other, Playable::Audio),
            Self::Stream(StreamEvent::SynthesisCompleted(completed)) => {
                Playable::Completed(completed.segment_id)
            }
            Self::Stream(StreamEvent::TurnOpened(opened)) => Playable::TurnOpened(opened.turn_id),
            Self::Stream(StreamEvent::GenerationSuperseded(superseded)) => {
                Playable::Superseded(superseded.generation_id)
            }
            Self::Stream(StreamEvent::GenerationStarted(started)) => Playable::Started {
                generation_id: started.generation_id,
                epoch: started.epoch,
            },
            Self::Audio(_) | Self::Stream(_) | Self::Receipt(_) | Self::Control(_) => {
                Playable::Other
            }
        }
    }

    fn receipt(segment_id: SegmentId, receipt: PlaybackReceipt) -> Self {
        Self::Receipt(Handoff::new((segment_id, receipt)))
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

/// The nodes of the cascade and the ports its run's implementations are given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CascadeIds {
    pub split: NodeId,
    pub vad: NodeId,
    pub policy: NodeId,
    pub stt: NodeId,
    pub input_gate: NodeId,
    pub turn_split: NodeId,
    pub llm: NodeId,
    pub text_split: NodeId,
    pub output_gate: NodeId,
    pub tts: NodeId,
    pub sink: NodeId,
    pub stt_ports: SttPorts,
    pub input_lanes: [GateLane; 2],
    pub llm_turns: PortId,
    pub output_lane: GateLane,
    pub speech_ports: SpeechPorts,
    pub sink_turns: PortId,
    pub llm_played: PortId,
    pub tts_playback: PortId,
}

impl CascadeIds {
    /// Every node, in the order of the module documentation's table.
    #[must_use]
    pub const fn nodes(&self) -> [NodeId; 11] {
        [
            self.split,
            self.vad,
            self.policy,
            self.stt,
            self.input_gate,
            self.turn_split,
            self.llm,
            self.text_split,
            self.output_gate,
            self.tts,
            self.sink,
        ]
    }
}

/// The cascade declared, validated and planned.
#[derive(Debug, Clone)]
pub struct LocalCascade {
    pub pipeline_id: PipelineId,
    pub planned: PlannedPipeline,
    pub ids: CascadeIds,
}

/// The profile the cascade is declared under and the adapters of its models.
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
) -> Result<LocalCascade, CascadeRefused> {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![Locality::SameProcess],
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
    let (pipeline_id, ids) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let split = pipeline.node("split", NodeRole::FanOut, Vec::new());
            let vad = pipeline.node("vad", NodeRole::Policy, Vec::new());
            let policy = pipeline.node("turn policy", NodeRole::Policy, Vec::new());
            let stt = pipeline.node("stt", NodeRole::Model, streaming.clone());
            let input_gate = pipeline.node("input gate", NodeRole::Policy, Vec::new());
            let turn_split = pipeline.node("turn split", NodeRole::FanOut, Vec::new());
            let llm = pipeline.node("agent", NodeRole::Model, vec![Capability::OutputStreaming]);
            let text_split = pipeline.node("text split", NodeRole::FanOut, Vec::new());
            let output_gate = pipeline.node("output gate", NodeRole::Policy, Vec::new());
            let tts = pipeline.node("tts", NodeRole::Model, streaming.clone());
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
            pipeline.expose(&microphone, AUDIO_INGRESS, capacity, pause);
            pipeline.expose(&committed, TRANSCRIPT_EGRESS, capacity, pause);
            pipeline.expose(&to_app, TEXT_EGRESS, capacity, pause);
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
                input_gate: input_gate.id(),
                turn_split: turn_split.id(),
                llm: llm.id(),
                text_split: text_split.id(),
                output_gate: output_gate.id(),
                tts: tts.id(),
                sink: sink.id(),
                stt_ports: SttPorts {
                    audio: stt_audio.id(),
                    turns: stt_turns.id(),
                    committed_text: committed.id(),
                    turns_out: turns_out.id(),
                    transcript: Some(transcript.id()),
                },
                input_lanes: [
                    GateLane {
                        input: gate_in.id(),
                        output: gate_out.id(),
                    },
                    GateLane {
                        input: gate_turns.id(),
                        output: gate_turns_out.id(),
                    },
                ],
                llm_turns: llm_turns.id(),
                sink_turns: sink_turns.id(),
                llm_played: llm_played.id(),
                tts_playback: tts_playback.id(),
                output_lane: GateLane {
                    input: held.id(),
                    output: passed.id(),
                },
                speech_ports: SpeechPorts {
                    text: speak.id(),
                    played: played_back.id(),
                    audio: spoken.id(),
                },
            }
        })
        .expect("the catalog holds the profile");
    let validated = declaration
        .validate(pipeline_id)
        .map_err(CascadeRefused::Invalid)?;
    let planned = declaration
        .plan(validated, &catalog)
        .map_err(CascadeRefused::Unplannable)?;
    Ok(LocalCascade {
        pipeline_id,
        planned,
        ids,
    })
}

/// The caller's side of a run's nodes.
#[derive(Debug, Clone)]
pub struct CascadeHandles {
    pub policy: TurnPolicyHandle,
    pub stt: SttHandle,
    pub input_gate: CommitGateHandle,
    pub llm: AgentHandle,
    pub output_gate: CommitGateHandle,
    pub tts: SpeechHandle,
    pub sink: AcknowledgingHandle,
}

/// The "committed input to first playable audio" of one turn: the time from the input gate
/// passing the turn's completion to the sink handing the first frame after it to the speaker,
/// `None` while no frame followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnMeasure {
    pub turn_id: TurnId,
    pub first_audio: Option<Duration>,
}

/// The "interruption to playback stop" of one interruption: the time from the sink taking the
/// opening of the turn `turn_id` to the speaker's stop returning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptionMeasure {
    pub turn_id: TurnId,
    pub stop: Duration,
}

impl CascadeHandles {
    /// The measure of every turn whose completion the input gate passed, in the order passed.
    #[must_use]
    pub fn committed_input_to_first_audio(&self) -> Vec<TurnMeasure> {
        let first_frames = self.sink.first_frames();
        self.input_gate
            .turns_passed()
            .into_iter()
            .map(|passed| TurnMeasure {
                turn_id: passed.turn_id,
                first_audio: first_frames
                    .iter()
                    .find(|first| first.at >= passed.at)
                    .map(|first| first.at.duration_since(passed.at)),
            })
            .collect()
    }

    /// The measure of every interruption, in the order the turns opened.
    #[must_use]
    pub fn interruption_to_playback_stop(&self) -> Vec<InterruptionMeasure> {
        self.sink
            .interruptions()
            .into_iter()
            .map(|interruption| InterruptionMeasure {
                turn_id: interruption.turn_id,
                stop: interruption.stop,
            })
            .collect()
    }
}

/// The implementations of one run's nodes, by node, and their handles.
pub struct CascadeRun {
    pub implementations: HashMap<NodeId, Box<dyn NodeImplementation<VoicePayload>>>,
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

impl LocalCascade {
    /// The implementations of the nodes of the run `run_id` over `mocks`, the turn policy's
    /// commands answered by `turns`, what the nodes emit recorded in `log`, and the speaker the
    /// default output of `host` plays, queuing `playout`.
    ///
    /// # Errors
    ///
    /// The [`DeviceError`] of opening the speaker; no node is made.
    pub fn nodes(
        &self,
        run_id: RunId,
        mocks: Mocks,
        turns: TurnStore,
        log: &RunLog,
        host: &dyn AudioHost,
        playout: PlayoutConfig,
    ) -> Result<CascadeRun, DeviceError> {
        self.nodes_with(run_id, mocks, |mock| mock, turns, log, host, playout)
    }

    /// As [`LocalCascade::nodes`], the TTS node synthesising on what `speak` makes of the mock
    /// synthesiser, such as a
    /// [`LateSynthesiser`](inference_testkit::cascade::late::LateSynthesiser) over it.
    ///
    /// # Errors
    ///
    /// The [`DeviceError`] of opening the speaker; no node is made.
    #[allow(clippy::too_many_arguments)]
    pub fn nodes_with<S>(
        &self,
        run_id: RunId,
        mocks: Mocks,
        speak: impl FnOnce(MockSynthesiser) -> S,
        turns: TurnStore,
        log: &RunLog,
        host: &dyn AudioHost,
        playout: PlayoutConfig,
    ) -> Result<CascadeRun, DeviceError>
    where
        S: Synthesiser + Send + 'static,
        S::Handle: Send + 'static,
    {
        let speaker = Speaker::open(host, &DeviceSelector::Default, playout)?;
        let ids = self.ids;
        let policy = TurnPolicy::new(run_id, turns);
        let stt = SttNode::new(run_id, mocks.recogniser, ids.stt_ports, log.clone());
        let input_gate = CommitGate::new(ids.input_lanes);
        let llm = AgentNode::new(
            run_id,
            mocks.generator,
            mocks.reply,
            ids.llm_turns,
            log.clone(),
        )
        .with_playback(ids.llm_played);
        let output_gate = CommitGate::new([ids.output_lane]);
        let tts = SpeechNode::new(
            run_id,
            speak(mocks.synthesiser),
            ids.speech_ports,
            log.clone(),
        )
        .with_playback(ids.tts_playback);
        let sink = AcknowledgingSink::new(run_id, speaker).with_turns(ids.sink_turns);
        let handles = CascadeHandles {
            policy: policy.handle(),
            stt: stt.handle(),
            input_gate: input_gate.handle(),
            llm: llm.handle(),
            output_gate: output_gate.handle(),
            tts: tts.handle(),
            sink: sink.handle(),
        };
        let implementations: HashMap<NodeId, Box<dyn NodeImplementation<VoicePayload>>> =
            HashMap::from([
                (
                    ids.split,
                    Box::new(FanOut::new()) as Box<dyn NodeImplementation<VoicePayload>>,
                ),
                (ids.vad, Box::new(VoiceActivityDetector::new(run_id, VAD))),
                (ids.policy, Box::new(policy)),
                (ids.stt, Box::new(stt)),
                (ids.input_gate, Box::new(input_gate)),
                (ids.turn_split, Box::new(FanOut::new())),
                (ids.llm, Box::new(llm)),
                (ids.text_split, Box::new(FanOut::new())),
                (ids.output_gate, Box::new(output_gate)),
                (ids.tts, Box::new(tts)),
                (ids.sink, Box::new(sink)),
            ]);
        Ok(CascadeRun {
            implementations,
            handles,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Handoff;

    #[test]
    fn a_handoff_and_every_copy_of_it_give_the_value_once() {
        let handoff = Handoff::new(String::from("frame"));
        let copy = handoff.clone();
        assert!(!copy.taken());
        assert_eq!(copy.take().as_deref(), Some("frame"));
        assert!(handoff.taken());
        assert_eq!(handoff.take(), None, "a copy holds nothing once taken");
        assert_eq!(copy.take(), None);
    }
}
