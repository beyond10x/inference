//! The nodes of one run of an instantiated recipe: each node given to `Recipe::with_node` built
//! by its implementation there, each other role-bound node built over its role's
//! binding in the stack, which owns the model, and every other node built by the recipe's
//! topology. [`Run::node`] is what [`Instantiation::open`](super::Instantiation::open) hands
//! `LiveSessions::open` for each node of the run.

use std::fmt;
use std::future;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::time::Duration;

use inference_core::endpoint::{
    ClosedToSenders, EndpointFuture, InputEndpoint, KnownContents, OutputEndpoint, SendError,
};
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::agent::AgentNode;
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::commit_gate::{CommitGate, GateLane};
use inference_pipeline::nodes::duplex::CallIds;
use inference_pipeline::nodes::duplex::DuplexPorts;
use inference_pipeline::nodes::llm::ModelNode;
use inference_pipeline::nodes::log::{RunLog, StreamEvent};
use inference_pipeline::nodes::speech::SpeechPorts;
use inference_pipeline::nodes::stt::SttPorts;
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::turn_policy::{CompleteTurnRefused, TurnCommands, TurnPolicy};
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::vad::VoiceActivityDetector;
use inference_pipeline::nodes::{
    AuthorityHandle, FanOut, ToolAuthority, ToolCallEvent, ToolExecutorNode,
};
use inference_pipeline::{
    Declaration, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeInput,
    NodeOutput, NodeRole, PipelineId, PlannedPipeline, PortId, USAGE_PORT,
};
use inference_text::{OpenTurn, Span, Turn, TurnId, TurnOpened};
#[cfg(feature = "audio-host")]
use inference_text::{TurnCompleted, TurnNotFound};
use inference_tools::{Tool, ToolCallId};
use tokio::sync::mpsc;

use super::topology::TOOL_EVENTS;
use super::{Recipe, Topology};
use crate::graph::Implementation;
use crate::stack::{BoundRecogniser, Synthesis, default_session_config};
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
    /// What the run's STT node recognises, once the run built one that is given no turns.
    recognised: Option<Recognised>,
    /// What the run's model node answers, once the run built one.
    replies: Option<Replies>,
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
            recognised: None,
            replies: None,
            #[cfg(feature = "audio-host")]
            speaker: instantiation.speaker.as_ref(),
            #[cfg(feature = "audio-host")]
            turns: Turns::default(),
            #[cfg(feature = "audio-host")]
            sink: None,
        }
    }

    /// The tool handles of a run with a tool loop, `native_voice`'s or that of `chat` given
    /// tools: its authority's hook, once the run built its authority, and its duplex node's call
    /// ids, once the run built a duplex node, each without the other; none when it built neither.
    pub(super) fn take_tools(&mut self) -> Option<super::Tools> {
        let tools = super::Tools {
            authority: self.handle.take(),
            call_ids: self.call_ids.take(),
        };
        (tools.authority.is_some() || tools.call_ids.is_some()).then_some(tools)
    }

    /// What the run's STT node recognises, when the run built one that is given no turns, as
    /// `transcribe`'s.
    pub(super) const fn take_recognised(&mut self) -> Option<Recognised> {
        self.recognised.take()
    }

    /// What the run's model node answers, when the run built one, as `chat`'s.
    pub(super) fn take_replies(&mut self) -> Option<Replies> {
        self.replies.take()
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
        let implementation = self.built(run_id, node)?;
        let model = match self.recipe.topology {
            Topology::NativeVoice => node.name() == "speech",
            Topology::Chat => node.name() == "agent" && self.port(node, "tool_call").is_ok(),
            _ => false,
        };
        if model && self.recipe.factory("authority").is_none() {
            let authority = self.authority(run_id)?;
            return Ok(Box::new(Closing {
                inner: implementation,
                authority,
            }));
        }
        Ok(implementation)
    }

    /// The implementation of `node` as given or bound, before the model node of a tool loop,
    /// `native_voice`'s speech node or the agent of `chat` given tools, is made to close its
    /// authority's hook.
    fn built(&mut self, run_id: RunId, node: &Node) -> Result<Implementation, Error> {
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
                let recogniser = binding.recogniser().ok_or_else(missing)?;
                let Ok(turns) = self.port(node, "turns") else {
                    return self.whole_input(run_id, node, recogniser);
                };
                let ports = SttPorts {
                    audio: self.port(node, "audio")?,
                    turns,
                    committed_text: self.port(node, "committed_text")?,
                    turns_out: self.port(node, "turns_out")?,
                    transcript: self.port(node, "transcript").ok(),
                };
                recogniser.node(run_id, ports, self.port(node, USAGE_PORT).ok(), log)
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
                        usage: self.port(node, USAGE_PORT).ok(),
                    }
                } else {
                    Synthesis::Plain {
                        usage: self.port(node, USAGE_PORT).ok(),
                    }
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

    /// The STT node of a pipeline that declares no port `turns` for it, as `transcribe`'s, where
    /// no node opens a turn: the node is given one turn of the run's own, opened as it starts and
    /// never completed, so it recognises the whole of its audio as that turn and ends the
    /// recognition at the end of the audio. A port `turns_out` the pipeline does not declare
    /// either is given an edge that keeps nothing.
    fn whole_input(
        &mut self,
        run_id: RunId,
        node: &Node,
        recogniser: &dyn BoundRecogniser,
    ) -> Result<Implementation, Error> {
        let turns = PortId::generate();
        let declared = self.port(node, "turns_out").ok();
        let turns_out = declared.unwrap_or_else(PortId::generate);
        let ports = SttPorts {
            audio: self.port(node, "audio")?,
            turns,
            committed_text: self.port(node, "committed_text")?,
            turns_out,
            transcript: self.port(node, "transcript").ok(),
        };
        let inner = recogniser.node(
            run_id,
            ports,
            self.port(node, USAGE_PORT).ok(),
            self.log.clone(),
        )?;
        let (turn, opened) = Turn::open_as(TurnId::generate(), OpenTurn { run_id });
        self.recognised = Some(Recognised {
            log: self.log.clone(),
            turn,
        });
        Ok(Box::new(WholeInput {
            inner,
            run_id,
            node_id: node.node_id(),
            turns,
            turns_out: declared.is_none().then_some(turns_out),
            opened: Some(opened),
        }))
    }

    /// The LLM node: the cascade's agent, one generation per turn, which keeps its replies when
    /// the cascade does not cut at a turn's opening; any other recipe's model node. Either sends
    /// the usage record of each model call on the node's port `usage` when the recipe declares
    /// one. The model node of `chat` given tools, whose node then declares the ports `tool_call`
    /// and `tool_result`, is given the recipe's tools and runs the tool loop of its generation.
    /// What a model node answers is kept for the instantiation ([`Replies`]).
    #[cfg_attr(
        not(feature = "audio-host"),
        allow(
            clippy::unnecessary_wraps,
            reason = "the cascade's agent needs its ports, which may be missing"
        )
    )]
    fn agent(
        &mut self,
        run_id: RunId,
        node: &Node,
        generator: inference_llm::Generator,
    ) -> Result<Implementation, Error> {
        #[cfg(feature = "audio-host")]
        if matches!(self.recipe.topology, Topology::VoiceCascade) {
            let mut agent = AgentNode::new(run_id, generator, reply, self.log.clone())
                .with_playback(self.port(node, "played")?)
                .with_instructions(self.recipe.instructions.clone().unwrap_or_default());
            if let Ok(usage) = self.port(node, USAGE_PORT) {
                agent = agent.with_usage(usage);
            }
            let cutting = self.speaker.is_some_and(|speaker| speaker.auto_interrupt);
            return Ok(if cutting {
                Box::new(agent)
            } else {
                Box::new(agent.keeping_replies())
            });
        }
        let mut model = ModelNode::<Payload>::carrying(run_id, generator, reply, self.log.clone())
            .with_instructions(self.recipe.instructions.clone().unwrap_or_default());
        let usage = self.port(node, USAGE_PORT).ok();
        if let Some(usage) = usage {
            model = model.with_usage(usage);
        }
        let tools = if matches!(self.recipe.topology, Topology::Chat) {
            self.port(node, "tool_call")
                .ok()
                .zip(self.port(node, "tool_result").ok())
        } else {
            None
        };
        if let Some((tool_call, tool_result)) = tools {
            model = model.with_tools(self.recipe.tools.tools().cloned(), tool_call, tool_result);
        }
        let replies = Replies::default();
        self.replies = Some(replies.clone());
        Ok(Box::new(Replying {
            inner: Box::new(model),
            replies,
            usage,
            tool_call: tools.map(|(tool_call, _)| tool_call),
            tool_result: tools.map(|(_, tool_result)| tool_result),
            tool_events: tools.and_then(|_| self.port(node, TOOL_EVENTS).ok()),
        }))
    }

    /// A node no role binds: a fan-out by its role, the tool nodes of `native_voice` and of
    /// `chat` given tools, the cascade's policy nodes and speaker by name.
    fn unbound(&mut self, run_id: RunId, node: &Node) -> Result<Implementation, Error> {
        if node.role() == NodeRole::FanOut {
            return Ok(Box::new(FanOut::new()));
        }
        if matches!(self.recipe.topology, Topology::NativeVoice | Topology::Chat) {
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
                    let tools = &self.recipe.tools;
                    let executor = ToolExecutorNode::new(
                        run_id,
                        Arc::new(tools.clone()),
                        tools.tool_ids(),
                        TOOL_DEADLINE,
                    );
                    if self.recipe.factory("authority").is_some() {
                        return Ok(Box::new(executor));
                    }
                    let handle = self.authority(run_id)?;
                    return Ok(Box::new(executor.answering_to(&handle)));
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

    /// The handle of the run `run_id`'s tool authority, building the authority
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

/// The model node of a tool loop, `native_voice`'s speech node or the agent of `chat` given
/// tools, which closes its authority's hook as the run drains it, so that a call held for
/// confirmation is denied and the session ends with no host answering it.
struct Closing {
    inner: Implementation,
    authority: AuthorityHandle,
}

impl fmt::Debug for Closing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Closing").finish_non_exhaustive()
    }
}

impl NodeImplementation<Payload> for Closing {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.inner.describe(node)
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.prepare()
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<Payload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.start(endpoints)
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.authority.close();
        self.inner.drain()
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.inner.cancel()
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.inner.stop()
    }
}

/// What the STT node of a run recognises when no node gives it turns: the run's log, which holds
/// every transcript event the node emitted in its envelope, and the one turn the node was given.
#[derive(Debug, Clone)]
pub(super) struct Recognised {
    log: RunLog,
    turn: Turn,
}

impl Recognised {
    /// The spans the node committed so far, in the order it committed them: each rebuilt from
    /// the events the run's log holds of it, by the steps the recogniser took, so it carries the
    /// identity, turn, stream, sequence and text the recogniser gave it. A commit whose
    /// hypothesis the log does not hold yields its span at the commit's own sequence.
    pub(super) fn spans(&self) -> Vec<Span> {
        let mut open: Vec<Span> = Vec::new();
        let mut committed = Vec::new();
        for envelope in self.log.envelopes() {
            let (run_id, stream_id) = (envelope.run_id, envelope.stream_id);
            let emit = |span_id, sequence, text| {
                Span::emit_as(span_id, &self.turn, run_id, stream_id, sequence, text)
                    .ok()
                    .map(|(span, _)| span)
            };
            match envelope.payload {
                StreamEvent::TranscriptHypothesis(heard) => {
                    open.extend(emit(heard.span_id, heard.sequence, heard.text));
                }
                StreamEvent::TranscriptRevision(revised) => {
                    if let Some(span) = open
                        .iter_mut()
                        .find(|span| span.span_id() == revised.span_id)
                        && let Ok((moved, _)) = span.revise(revised.text)
                    {
                        *span = moved;
                    }
                }
                StreamEvent::TranscriptCommitted(heard) => {
                    let known = open
                        .iter()
                        .position(|span| span.span_id() == heard.span_id)
                        .map(|at| open.remove(at));
                    let span = known.or_else(|| emit(heard.span_id, envelope.sequence, heard.text));
                    committed.extend(
                        span.and_then(|span| span.commit().ok())
                            .map(|(span, _)| span),
                    );
                }
                _ => {}
            }
        }
        committed
    }
}

/// An STT node that is given no turns by its pipeline: started, it hands the node it wraps an
/// edge at its port `turns` that holds one `TurnOpened` and then ends, and, when the pipeline
/// declares no `turns_out` either, an edge at that port that keeps nothing.
struct WholeInput {
    inner: Implementation,
    run_id: RunId,
    node_id: NodeId,
    turns: PortId,
    /// The port `turns_out` when the pipeline declares none.
    turns_out: Option<PortId>,
    opened: Option<TurnOpened>,
}

impl fmt::Debug for WholeInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WholeInput").finish_non_exhaustive()
    }
}

/// An input edge that holds one envelope and then ends.
struct Once(Option<EventEnvelope<Payload>>);

impl InputEndpoint<Payload> for Once {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        Box::pin(future::ready(self.0.take()))
    }
}

/// An output edge that takes every envelope and keeps none.
struct Unread;

impl OutputEndpoint<Payload> for Unread {
    fn send(
        &self,
        _: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        Box::pin(future::ready(Ok(())))
    }
}

impl NodeImplementation<Payload> for WholeInput {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.inner.describe(node)
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.prepare()
    }

    fn start(
        &mut self,
        mut endpoints: NodeEndpoints<Payload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let opened = self.opened.take().map(|opened| {
            let event = StreamEvent::TurnOpened(opened);
            EventEnvelope {
                schema_version: 1,
                event_id: EventId::generate(),
                run_id: self.run_id,
                session_id: None,
                stream_id: StreamId::generate(),
                sequence: 1,
                producer: self.node_id,
                port: "turns".to_owned(),
                kind: event.name().to_owned(),
                generation_id: None,
                timestamp: 0,
                timebase: Timebase::WallTime,
                causation_id: None,
                payload: Payload::Stream(event),
            }
        });
        endpoints.inputs.push(NodeInput {
            port_id: self.turns,
            endpoint: Box::new(Once(opened)),
        });
        if let Some(turns_out) = self.turns_out {
            endpoints.outputs.push(NodeOutput {
                port_id: turns_out,
                endpoint: Box::new(Unread),
            });
        }
        self.inner.start(endpoints)
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.drain()
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.inner.cancel()
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.inner.stop()
    }
}

/// One step of what a run's model node answers, as a reader beside the session takes it
/// (`Instantiation::replies`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplyPart {
    /// The text of one delta the node sent, as the model streamed it.
    Delta(String),
    /// The model refused to answer a turn; the text is its refusal, which is no delta.
    Refused(String),
    /// The reply is whole: every input the node took is answered and it awaits no tool call.
    /// `ended` says the node's generation ended, completed, superseded or cancelled, as when its
    /// tool calls reached the bound of the tool loop: it answers no later input.
    End { ended: bool },
}

/// What the wrapped model node took and sent, as its endpoints saw it.
#[derive(Default)]
struct Answering {
    reader: Option<mpsc::UnboundedSender<ReplyPart>>,
    /// The calls the node proposed whose outcome it has not taken.
    awaited: Vec<ToolCallId>,
    /// How many inputs the node took, and how many of them an [`ReplyPart::End`] covered.
    taken: u64,
    answered: u64,
    ended: bool,
}

/// What the model node of a run answers, told to one reader as the node's endpoints see it:
/// clones share one state. The node answers on its own task, one input after another, so the
/// reader gets the deltas of a reply in order and its end after them.
#[derive(Clone, Default)]
pub(super) struct Replies(Arc<Mutex<Answering>>);

impl fmt::Debug for Replies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Replies").finish_non_exhaustive()
    }
}

impl Replies {
    fn answering(&self) -> std::sync::MutexGuard<'_, Answering> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A reader of what the node answers from now on; it replaces the reader before it.
    pub(super) fn listen(&self) -> mpsc::UnboundedReceiver<ReplyPart> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.answering().reader = Some(sender);
        receiver
    }

    fn tell(answering: &Answering, part: ReplyPart) {
        if let Some(reader) = &answering.reader {
            let _ = reader.send(part);
        }
    }

    /// The node sends `envelope` on its text port.
    fn sends(&self, envelope: &EventEnvelope<Payload>) {
        let Payload::Stream(event) = &envelope.payload else {
            return;
        };
        let mut answering = self.answering();
        match event {
            StreamEvent::TextDelta(delta) => {
                Self::tell(&answering, ReplyPart::Delta(delta.text.clone()));
            }
            StreamEvent::GenerationRefused(refused) => {
                Self::tell(&answering, ReplyPart::Refused(refused.text.clone()));
            }
            StreamEvent::GenerationCompleted(_)
            | StreamEvent::GenerationSuperseded(_)
            | StreamEvent::GenerationCancelled(_) => answering.ended = true,
            _ => {}
        }
    }

    /// The node sends `envelope` on its port `tool_call`: a proposal is a call it awaits.
    fn proposes(&self, envelope: &EventEnvelope<Payload>) {
        if let Payload::Tool(update) = &envelope.payload
            && matches!(update.event, ToolCallEvent::Proposed(_))
        {
            self.answering().awaited.push(update.call.call_id());
        }
    }

    /// The node takes `envelope` from an input edge, one into its port `tool_result` when
    /// `result`: a committed text is an input it answers, and a call's result, failure or
    /// rejection ends the wait for that call.
    fn takes(&self, envelope: &EventEnvelope<Payload>, result: bool) {
        let mut answering = self.answering();
        match &envelope.payload {
            Payload::Stream(StreamEvent::TranscriptCommitted(_)) if !result => {
                answering.taken += 1;
            }
            Payload::Tool(update)
                if result
                    && matches!(
                        update.event,
                        ToolCallEvent::Result(_)
                            | ToolCallEvent::Failed(_)
                            | ToolCallEvent::Rejected(_)
                    ) =>
            {
                let call_id = update.call.call_id();
                answering.awaited.retain(|awaited| *awaited != call_id);
            }
            _ => {}
        }
    }

    /// The node asks an input edge for its next envelope, which it does only between the steps
    /// of its exchange: when it awaits no call, every input it took is answered, since an input
    /// it takes while it awaits one is answered before it asks again. Tells the reader the end
    /// of the reply once for the inputs taken since the last.
    fn asks(&self) {
        let mut answering = self.answering();
        if answering.awaited.is_empty() && answering.taken > answering.answered {
            answering.answered = answering.taken;
            Self::tell(
                &answering,
                ReplyPart::End {
                    ended: answering.ended,
                },
            );
        }
    }
}

/// An input edge of the wrapped model node, telling [`Replies`] what the node asks and takes.
struct Asked {
    inner: Box<dyn InputEndpoint<Payload>>,
    replies: Replies,
    /// Whether the edge arrives at the node's port `tool_result`.
    result: bool,
}

impl InputEndpoint<Payload> for Asked {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        let (replies, result) = (self.replies.clone(), self.result);
        let mut inner = self.inner.recv();
        Box::pin(future::poll_fn(move |cx| {
            replies.asks();
            let received = inner.as_mut().poll(cx);
            if let Poll::Ready(Some(envelope)) = &received {
                replies.takes(envelope, result);
            }
            received
        }))
    }

    fn known_contents(&self) -> KnownContents {
        self.inner.known_contents()
    }

    fn close_to_senders(&mut self) -> ClosedToSenders {
        self.inner.close_to_senders()
    }
}

/// An output edge of the wrapped model node, telling [`Replies`] what the node sends on it.
struct Answered {
    inner: Box<dyn OutputEndpoint<Payload>>,
    replies: Replies,
    /// Whether the edge leaves the node's port `tool_call`, rather than its text port.
    proposals: bool,
    /// The edge leaving the node's port [`TOOL_EVENTS`], held by an edge that leaves its port
    /// `tool_call`: what the node sends on `tool_call`, a proposed call or the end of its
    /// calls, is sent there first, so the session tells a call before the authority takes it.
    events: Option<Arc<dyn OutputEndpoint<Payload>>>,
}

impl OutputEndpoint<Payload> for Answered {
    fn send(
        &self,
        envelope: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        if self.proposals {
            self.replies.proposes(&envelope);
        } else {
            self.replies.sends(&envelope);
        }
        let Some(events) = &self.events else {
            return self.inner.send(envelope);
        };
        let told = EventEnvelope {
            event_id: EventId::generate(),
            port: TOOL_EVENTS.to_owned(),
            causation_id: None,
            ..envelope.clone()
        };
        Box::pin(async move {
            // A session that no longer takes its events does not keep the call from its tool.
            let _ = events.send(told).await;
            self.inner.send(envelope).await
        })
    }
}

/// A model node whose edges tell [`Replies`] what it answers: every input edge, every edge
/// leaving its text port and every edge leaving its port `tool_call`; its `usage` port's edges
/// are handed on as they are. The edges leaving its port [`TOOL_EVENTS`], which the node itself
/// does not know, are kept from it and given to the edges leaving `tool_call`.
struct Replying {
    inner: Implementation,
    replies: Replies,
    usage: Option<PortId>,
    tool_call: Option<PortId>,
    tool_result: Option<PortId>,
    tool_events: Option<PortId>,
}

impl fmt::Debug for Replying {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Replying").finish_non_exhaustive()
    }
}

impl NodeImplementation<Payload> for Replying {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.inner.describe(node)
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.prepare()
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<Payload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let inputs = endpoints
            .inputs
            .into_iter()
            .map(|input| NodeInput {
                port_id: input.port_id,
                endpoint: Box::new(Asked {
                    inner: input.endpoint,
                    replies: self.replies.clone(),
                    result: self.tool_result == Some(input.port_id),
                }),
            })
            .collect();
        let (told, outputs): (Vec<_>, Vec<_>) = endpoints
            .outputs
            .into_iter()
            .partition(|output| self.tool_events == Some(output.port_id));
        let events: Option<Arc<dyn OutputEndpoint<Payload>>> = told
            .into_iter()
            .next()
            .map(|output| Arc::from(output.endpoint));
        let outputs = outputs
            .into_iter()
            .map(|output| {
                if self.usage == Some(output.port_id) {
                    return output;
                }
                let proposals = self.tool_call == Some(output.port_id);
                NodeOutput {
                    port_id: output.port_id,
                    endpoint: Box::new(Answered {
                        inner: output.endpoint,
                        replies: self.replies.clone(),
                        proposals,
                        events: events.clone().filter(|_| proposals),
                    }),
                }
            })
            .collect();
        self.inner.start(NodeEndpoints { inputs, outputs })
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.drain()
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.inner.cancel()
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.inner.stop()
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
