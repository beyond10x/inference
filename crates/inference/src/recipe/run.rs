//! The nodes of one run of an instantiated recipe: each node given to `Recipe::with_node` built
//! by its implementation there, each other role-bound node built over its role's
//! binding in the stack, which owns the model, and every other node built by the recipe's
//! topology. [`Run::node`] is what [`Instantiation::open`](super::Instantiation::open) hands
//! `LiveSessions::open` for each node of the run.

use std::fmt;
use std::future;
use std::sync::Arc;
#[cfg(feature = "audio-host")]
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use inference_core::id::RunId;
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::agent::AgentNode;
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::commit_gate::{CommitGate, GateLane};
use inference_pipeline::nodes::duplex::CallIds;
use inference_pipeline::nodes::duplex::DuplexPorts;
use inference_pipeline::nodes::llm::ModelNode;
use inference_pipeline::nodes::log::RunLog;
use inference_pipeline::nodes::speech::SpeechPorts;
use inference_pipeline::nodes::stt::SttPorts;
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::turn_policy::{CompleteTurnRefused, TurnCommands, TurnPolicy};
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::vad::VoiceActivityDetector;
use inference_pipeline::nodes::{AuthorityHandle, FanOut, ToolAuthority, ToolExecutorNode};
use inference_pipeline::{
    Declaration, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeRole,
    PipelineId, PlannedPipeline, PortId,
};
#[cfg(feature = "audio-host")]
use inference_text::{OpenTurn, Turn, TurnCompleted, TurnId, TurnNotFound, TurnOpened};
use inference_tools::Tool;

use super::{Recipe, Topology};
use crate::graph::Implementation;
use crate::stack::{Synthesis, default_session_config};
use crate::{Error, Payload, Role, Stack};

/// How long `native_voice`'s executor gives each tool call from its start.
const TOOL_DEADLINE: Duration = Duration::from_secs(30);

/// The user item a model node is asked: the input text as it came.
fn reply(text: &str) -> String {
    text.to_owned()
}

/// The builder of one run's nodes.
pub(super) struct Run<'i> {
    recipe: &'i Recipe,
    stack: &'i Stack,
    declaration: &'i Declaration,
    planned: &'i PlannedPipeline,
    pipeline_id: PipelineId,
    log: RunLog,
    /// `native_voice`'s authority, built by whichever of its two tool nodes is built first, until
    /// its own node takes it.
    authority: Option<ToolAuthority>,
    handle: Option<AuthorityHandle>,
    call_ids: Option<CallIds>,
    #[cfg(feature = "audio-host")]
    speaker: Option<&'i super::speaker::Speaker>,
    #[cfg(feature = "audio-host")]
    turns: Turns,
    #[cfg(feature = "audio-host")]
    sink: Option<inference_audio_device::acknowledging::AcknowledgingHandle>,
}

impl<'i> Run<'i> {
    pub(super) fn new(instantiation: &'i super::Instantiation, stack: &'i Stack) -> Self {
        Self {
            recipe: &instantiation.recipe,
            stack,
            declaration: &instantiation.declaration,
            planned: &instantiation.planned,
            pipeline_id: instantiation.pipeline_id,
            log: RunLog::new(),
            authority: None,
            handle: None,
            call_ids: None,
            #[cfg(feature = "audio-host")]
            speaker: instantiation.speaker.as_ref(),
            #[cfg(feature = "audio-host")]
            turns: Turns::default(),
            #[cfg(feature = "audio-host")]
            sink: None,
        }
    }

    /// The tool handles of a `native_voice` run, once its duplex node and its authority were
    /// built.
    pub(super) fn take_tools(&mut self) -> Option<super::Tools> {
        Some(super::Tools {
            authority: self.handle.take()?,
            call_ids: self.call_ids.take()?,
        })
    }

    /// The handle of the run's speaker sink, once it was built.
    #[cfg(feature = "audio-host")]
    pub(super) fn into_sink(
        self,
    ) -> Option<inference_audio_device::acknowledging::AcknowledgingHandle> {
        self.sink
    }

    /// The implementation of `node` in the run `run_id`.
    ///
    /// # Errors
    ///
    /// [`Error::Layer`] when the node is role-bound to a role whose binding in the stack is not
    /// the adapter the recipe was planned with, when a port the node's implementation needs is
    /// not declared, or when the recipe builds no implementation for the node; the refusal of a
    /// binding's factory, such as a deferred recogniser that did not open.
    pub(super) fn node(&mut self, run_id: RunId, node: &Node) -> Result<Implementation, Error> {
        if let Some(factory) = self.recipe.factory(node.name()) {
            return Ok(factory(run_id, node));
        }
        let role = self
            .recipe
            .roles
            .iter()
            .find(|(name, _)| name == node.name())
            .map(|(_, role)| *role);
        match role {
            Some(role) => self.bound(run_id, node, role),
            None => self.unbound(run_id, node),
        }
    }

    /// The node role-bound to `role`, over that role's binding.
    fn bound(&mut self, run_id: RunId, node: &Node, role: Role) -> Result<Implementation, Error> {
        let binding = self.stack.adapter(role).ok_or_else(|| Error::RoleUnbound {
            role: role.as_str().to_owned(),
        })?;
        let planned = self
            .declaration
            .bindings()
            .find(|bound| bound.node_id() == node.node_id())
            .map(inference_pipeline::Binding::adapter_id);
        if planned != Some(binding.adapter().adapter_id()) {
            return Err(Error::Layer {
                message: format!(
                    "the node `{}` of the recipe `{}` was planned with another adapter than the \
                     stack binds to the role {role}: open the instantiation over the stack it was \
                     instantiated over",
                    node.name(),
                    self.recipe.name
                ),
                source: None,
            });
        }
        let log = self.log.clone();
        let missing = || Error::Layer {
            message: format!("the {role} binding holds no model of its role"),
            source: None,
        };
        match role {
            Role::Llm => {
                let generator = binding.generator().ok_or_else(missing)?.clone();
                self.agent(run_id, node, generator)
            }
            Role::Stt => {
                let ports = SttPorts {
                    audio: self.port(node, "audio")?,
                    turns: self.port(node, "turns")?,
                    committed_text: self.port(node, "committed_text")?,
                    turns_out: self.port(node, "turns_out")?,
                    transcript: self.port(node, "transcript").ok(),
                };
                binding
                    .recogniser()
                    .ok_or_else(missing)?
                    .node(run_id, ports, log)
            }
            Role::Tts => {
                let synthesis = if matches!(self.recipe.topology, Topology::VoiceCascade) {
                    Synthesis::Speech {
                        ports: SpeechPorts {
                            text: self.port(node, "text")?,
                            played: self.port(node, "played")?,
                            audio: self.port(node, "audio")?,
                        },
                        playback: self.port(node, "playback")?,
                    }
                } else {
                    Synthesis::Plain
                };
                Ok(binding
                    .synthesiser()
                    .ok_or_else(missing)?
                    .node(run_id, synthesis, log))
            }
            Role::Speech => {
                let ports = DuplexPorts {
                    audio: self.port(node, "audio")?,
                    control: self.port(node, "control")?,
                    events: self.port(node, "events")?,
                    tool_call: self.port(node, "tool_call").ok(),
                    tool_result: self.port(node, "tool_result").ok(),
                };
                let mut config = self
                    .stack
                    .session_config()
                    .cloned()
                    .unwrap_or_else(default_session_config);
                let native = matches!(self.recipe.topology, Topology::NativeVoice);
                let tools: Vec<Tool> = if native {
                    self.recipe.tools.tools().cloned().collect()
                } else {
                    Vec::new()
                };
                for tool in &tools {
                    if !config
                        .tools
                        .iter()
                        .any(|offered| offered.name() == tool.name())
                    {
                        config.tools.push(tool.clone());
                    }
                }
                let (implementation, call_ids) = binding
                    .speech_model()
                    .ok_or_else(missing)?
                    .node(run_id, config, ports, tools, log);
                if native {
                    self.call_ids = Some(call_ids);
                }
                Ok(implementation)
            }
        }
    }

    /// The LLM node: the cascade's agent, one generation per turn, which keeps its replies when
    /// the cascade does not cut at a turn's opening; any other recipe's model node.
    #[cfg_attr(
        not(feature = "audio-host"),
        allow(
            clippy::unnecessary_wraps,
            reason = "the cascade's agent needs its ports, which may be missing"
        )
    )]
    fn agent(
        &self,
        run_id: RunId,
        node: &Node,
        generator: inference_llm::Generator,
    ) -> Result<Implementation, Error> {
        #[cfg(feature = "audio-host")]
        if matches!(self.recipe.topology, Topology::VoiceCascade) {
            let agent = AgentNode::new(
                run_id,
                generator,
                reply,
                self.port(node, "turns")?,
                self.log.clone(),
            )
            .with_playback(self.port(node, "played")?);
            let cutting = self.speaker.is_some_and(|speaker| speaker.auto_interrupt);
            return Ok(if cutting {
                Box::new(agent)
            } else {
                Box::new(agent.keeping_replies())
            });
        }
        let _ = node;
        Ok(Box::new(ModelNode::<Payload>::carrying(
            run_id,
            generator,
            reply,
            self.log.clone(),
        )))
    }

    /// A node no role binds: a fan-out by its role, the cascade's policy nodes and speaker by
    /// name.
    fn unbound(&mut self, run_id: RunId, node: &Node) -> Result<Implementation, Error> {
        if node.role() == NodeRole::FanOut {
            return Ok(Box::new(FanOut::new()));
        }
        if matches!(self.recipe.topology, Topology::NativeVoice) {
            match node.name() {
                "authority" => {
                    self.authority(run_id)?;
                    let authority = self.authority.take().ok_or_else(|| Error::Layer {
                        message: format!(
                            "the recipe `{}` builds its node `authority` once per run",
                            self.recipe.name
                        ),
                        source: None,
                    })?;
                    return Ok(Box::new(authority));
                }
                "executor" => {
                    let handle = self.authority(run_id)?;
                    let tools = &self.recipe.tools;
                    return Ok(Box::new(
                        ToolExecutorNode::new(
                            run_id,
                            Arc::new(tools.clone()),
                            tools.tool_ids(),
                            TOOL_DEADLINE,
                        )
                        .answering_to(&handle),
                    ));
                }
                _ => {}
            }
        }
        #[cfg(feature = "audio-host")]
        if matches!(self.recipe.topology, Topology::VoiceCascade) {
            match node.name() {
                "vad" => {
                    return Ok(Box::new(VoiceActivityDetector::new(
                        run_id,
                        self.recipe.vad,
                    )));
                }
                "turn policy" => {
                    return Ok(Box::new(TurnPolicy::new(run_id, self.turns.clone())));
                }
                "input gate" => {
                    return Ok(Box::new(CommitGate::new([
                        self.lane(node, "input", "output")?,
                        self.lane(node, "turns", "turns_out")?,
                    ])));
                }
                "output gate" => {
                    return Ok(Box::new(CommitGate::new([
                        self.lane(node, "input", "output")?
                    ])));
                }
                "speaker" => return self.speaker(run_id, node),
                _ => {}
            }
        }
        let _ = run_id;
        Err(Error::Layer {
            message: format!(
                "the recipe `{}` builds no implementation for its node `{}`",
                self.recipe.name,
                node.name()
            ),
            source: None,
        })
    }

    /// The handle of `native_voice`'s authority in the run `run_id`, building the authority
    /// first if no node of the run built it yet: deciding calls of the recipe's tools, bounded
    /// by the feedback loop into its node, and sending every rejection on its port `rejected`.
    fn authority(&mut self, run_id: RunId) -> Result<AuthorityHandle, Error> {
        if let Some(handle) = &self.handle {
            return Ok(handle.clone());
        }
        let node = self
            .declaration
            .nodes()
            .find(|node| node.pipeline_id() == self.pipeline_id && node.name() == "authority")
            .ok_or_else(|| Error::Layer {
                message: format!(
                    "the recipe `{}` declares no node `authority`",
                    self.recipe.name
                ),
                source: None,
            })?;
        let rejected = self.port(node, "rejected")?;
        let authority = ToolAuthority::new(run_id, self.recipe.tools.tools().cloned())
            .bounded_by(self.declaration, self.planned, node.node_id())
            .map_err(|error| Error::Layer {
                message: format!(
                    "the recipe `{}` bounds no tool loop into its authority",
                    self.recipe.name
                ),
                source: Some(Box::new(error)),
            })?
            .rejecting_on(rejected);
        let handle = authority.handle();
        self.authority = Some(authority);
        self.handle = Some(handle.clone());
        Ok(handle)
    }

    /// The cascade's speaker: the reference cascade's sink over the route the cascade was
    /// instantiated with, its handle kept for the instantiation.
    #[cfg(feature = "audio-host")]
    fn speaker(&mut self, run_id: RunId, node: &Node) -> Result<Implementation, Error> {
        let speaker = self.speaker.ok_or_else(|| Error::Layer {
            message: format!(
                "the recipe `{}` plays its replies through an agent's audio route, and it was \
                 instantiated without one: open it through an agent",
                self.recipe.name
            ),
            source: None,
        })?;
        let sink = super::speaker::sink(
            run_id,
            &speaker.route,
            self.port(node, "turns")?,
            speaker.auto_interrupt,
        );
        self.sink = Some(sink.handle());
        Ok(Box::new(sink))
    }

    #[cfg(feature = "audio-host")]
    fn lane(&self, node: &Node, input: &str, output: &str) -> Result<GateLane, Error> {
        Ok(GateLane {
            input: self.port(node, input)?,
            output: self.port(node, output)?,
        })
    }

    /// The port `name` of `node`.
    fn port(&self, node: &Node, name: &str) -> Result<PortId, Error> {
        self.declaration
            .ports()
            .find(|port| {
                port.pipeline_id() == self.pipeline_id
                    && port.node_id() == node.node_id()
                    && port.name() == name
            })
            .map(inference_pipeline::Port::port_id)
            .ok_or_else(|| Error::Layer {
                message: format!(
                    "the node `{}` of the recipe `{}` declares no port `{name}`, which its \
                     implementation needs",
                    node.name(),
                    self.recipe.name
                ),
                source: None,
            })
    }
}

/// The turns of one cascade run, answering its turn policy: a turn opens under a fresh identity
/// and completes once. Clones share one store.
#[cfg(feature = "audio-host")]
#[derive(Debug, Clone, Default)]
struct Turns(Arc<Mutex<Vec<Turn>>>);

#[cfg(feature = "audio-host")]
impl TurnCommands for Turns {
    fn open_turn(&mut self, input: OpenTurn) -> Result<TurnOpened, String> {
        let (turn, opened) = Turn::open_as(TurnId::generate(), input);
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(turn);
        Ok(opened)
    }

    fn complete_turn(&mut self, turn_id: TurnId) -> Result<TurnCompleted, CompleteTurnRefused> {
        let mut turns = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = turns
            .iter_mut()
            .find(|turn| turn.turn_id() == turn_id)
            .ok_or(CompleteTurnRefused::Unknown(TurnNotFound { turn_id }))?;
        let (moved, completed) = slot.complete().map_err(CompleteTurnRefused::Conflict)?;
        *slot = moved;
        Ok(completed)
    }
}

/// A node the run could not build: it fails at its preparation with the reason, so the run
/// does not open.
pub(super) struct Refused(pub(super) String);

impl fmt::Debug for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Refused").field(&self.0).finish()
    }
}

impl NodeImplementation<Payload> for Refused {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Err(NodeError::new(self.0.clone()))))
    }

    fn start(&mut self, _: NodeEndpoints<Payload>) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Err(NodeError::new(self.0.clone()))))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use std::collections::{BTreeMap, VecDeque};
    use std::future::{self, Future};
    use std::num::{NonZeroU16, NonZeroU32};
    use std::pin::pin;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread::{self, Thread};
    use std::time::Duration;

    use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
    use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
    use inference_core::envelope::{EventEnvelope, EventId, Timebase};
    use inference_core::id::{NodeId, RunId, StreamId};
    use inference_pipeline::nodes::vad::{SpeechActivity, VadConfig};
    use inference_pipeline::{NodeEndpoints, NodeInput, NodeOutput};

    use super::Run;
    use crate::recipe::VAD;
    use crate::{Binding, Env, Payload, Recipe, Role, Stack};

    /// Samples in one frame of the utterance: 20 ms at 16 kHz.
    const FRAME: u32 = 320;

    struct Unpark(Thread);

    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut future = pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
                return output;
            }
            thread::park_timeout(Duration::from_millis(10));
        }
    }

    /// An input edge holding the envelopes it hands out, closed once they are taken.
    struct Feed(VecDeque<EventEnvelope<Payload>>);

    impl InputEndpoint<Payload> for Feed {
        fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
            Box::pin(future::ready(self.0.pop_front()))
        }
    }

    /// An output edge keeping the kind of each envelope sent on it.
    #[derive(Clone, Default)]
    struct Kinds(Arc<Mutex<Vec<String>>>);

    impl OutputEndpoint<Payload> for Kinds {
        fn send(
            &self,
            envelope: EventEnvelope<Payload>,
        ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(envelope.kind);
            Box::pin(future::ready(Ok(())))
        }
    }

    /// Half a second of a tone at 0.5 of full scale, then half a second of silence, as 16 kHz
    /// mono frames.
    fn utterance() -> Vec<EventEnvelope<Payload>> {
        let format = AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).expect("a rate"),
            channels: NonZeroU16::MIN,
        };
        (0_u64..50)
            .map(|index| {
                let level = if index < 25 { 16_384_i16 } else { 0 };
                let frame = AudioFrame {
                    format,
                    position: SampleClockPosition::new(index * u64::from(FRAME))
                        .expect("a position"),
                    sample_count: FRAME,
                    data: level.to_le_bytes().repeat(FRAME as usize),
                };
                EventEnvelope {
                    schema_version: 1,
                    event_id: EventId::generate(),
                    run_id: RunId::generate(),
                    session_id: None,
                    stream_id: StreamId::generate(),
                    sequence: i64::try_from(index).expect("a small index") + 1,
                    producer: NodeId::generate(),
                    port: "audio".to_owned(),
                    kind: "inference.audio.AudioChunk".to_owned(),
                    generation_id: None,
                    timestamp: 0,
                    timebase: Timebase::AudioFrameTime,
                    causation_id: None,
                    payload: Payload::Audio(frame),
                }
            })
            .collect()
    }

    /// The `SpeechStarted` events the `vad` node of `recipe`'s run, built as an open builds it,
    /// sends over [`utterance`].
    fn speech_started(recipe: &Recipe) -> usize {
        let stack = [Role::Stt, Role::Llm, Role::Tts]
            .into_iter()
            .fold(Stack::new(), |stack, role| {
                stack.bind(Binding::named(role, "mock", BTreeMap::new()))
            })
            .resolve_with(&Env::new())
            .expect("the stack resolves");
        let instantiation = recipe
            .instantiate(&stack)
            .expect("the cascade plans over the mocks");
        let node = instantiation
            .declaration
            .nodes()
            .find(|node| node.name() == "vad")
            .expect("the cascade has a detector")
            .clone();
        let mut run = Run::new(&instantiation, &stack);
        let audio = run.port(&node, "audio").expect("the detector hears audio");
        let activity = run
            .port(&node, "activity")
            .expect("the detector tells activity");
        let mut detector = run
            .node(RunId::generate(), &node)
            .expect("the run builds its detector");
        let kinds = Kinds::default();
        block_on(detector.describe(&node));
        block_on(detector.prepare()).expect("the detector prepares");
        block_on(detector.start(NodeEndpoints {
            inputs: vec![NodeInput {
                port_id: audio,
                endpoint: Box::new(Feed(utterance().into())),
            }],
            outputs: vec![NodeOutput {
                port_id: activity,
                endpoint: Box::new(kinds.clone()),
            }],
        }))
        .expect("the detector starts");
        block_on(detector.drain()).expect("the detector drains");
        block_on(detector.stop());
        let sent = kinds.0.lock().unwrap_or_else(PoisonError::into_inner);
        sent.iter()
            .filter(|kind| *kind == SpeechActivity::STARTED)
            .count()
    }

    #[test]
    fn the_cascades_detector_runs_with_the_recipes_vad() {
        assert_eq!(
            speech_started(&Recipe::voice_cascade()),
            1,
            "the default VAD"
        );
        let deaf = Recipe::voice_cascade().vad(VadConfig {
            threshold: 0.9,
            ..VAD
        });
        assert_eq!(speech_started(&deaf), 0, "a threshold above the tone");
    }
}
