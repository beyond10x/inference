//! The nodes of one run of an instantiated recipe: each role-bound node built over its role's
//! binding in the stack, which owns the model, and every other node built by the recipe's
//! topology. [`Run::node`] is what [`Instantiation::open`](super::Instantiation::open) hands
//! `LiveSessions::open` for each node of the run.

use std::fmt;
use std::future;
#[cfg(feature = "audio-host")]
use std::sync::{Arc, Mutex, PoisonError};
#[cfg(feature = "audio-host")]
use std::time::Duration;

use inference_core::id::RunId;
use inference_pipeline::nodes::FanOut;
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::agent::AgentNode;
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::commit_gate::{CommitGate, GateLane};
use inference_pipeline::nodes::duplex::DuplexPorts;
use inference_pipeline::nodes::llm::ModelNode;
use inference_pipeline::nodes::log::RunLog;
use inference_pipeline::nodes::speech::SpeechPorts;
use inference_pipeline::nodes::stt::SttPorts;
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::turn_policy::{CompleteTurnRefused, TurnCommands, TurnPolicy};
#[cfg(feature = "audio-host")]
use inference_pipeline::nodes::vad::{VadConfig, VoiceActivityDetector};
use inference_pipeline::{
    Declaration, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeRole,
    PipelineId, PortId,
};
#[cfg(feature = "audio-host")]
use inference_text::{OpenTurn, Turn, TurnCompleted, TurnId, TurnNotFound, TurnOpened};

use super::{Recipe, Topology};
use crate::graph::Implementation;
use crate::stack::{Synthesis, default_session_config};
use crate::{Error, Payload, Role, Stack};

/// The detector of `voice_cascade`: the reference cascade's values.
#[cfg(feature = "audio-host")]
const VAD: VadConfig = VadConfig {
    threshold: 0.05,
    attack: Duration::from_millis(30),
    hold: Duration::from_millis(200),
};

/// The user item a model node is asked: the input text as it came.
fn reply(text: &str) -> String {
    text.to_owned()
}

/// The builder of one run's nodes.
pub(super) struct Run<'i> {
    recipe: &'i Recipe,
    stack: &'i Stack,
    declaration: &'i Declaration,
    pipeline_id: PipelineId,
    log: RunLog,
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
            pipeline_id: instantiation.pipeline_id,
            log: RunLog::new(),
            #[cfg(feature = "audio-host")]
            speaker: instantiation.speaker.as_ref(),
            #[cfg(feature = "audio-host")]
            turns: Turns::default(),
            #[cfg(feature = "audio-host")]
            sink: None,
        }
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
                let config = self
                    .stack
                    .session_config()
                    .cloned()
                    .unwrap_or_else(default_session_config);
                Ok(binding
                    .speech_model()
                    .ok_or_else(missing)?
                    .node(run_id, config, ports, log))
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
        #[cfg(feature = "audio-host")]
        if matches!(self.recipe.topology, Topology::VoiceCascade) {
            match node.name() {
                "vad" => return Ok(Box::new(VoiceActivityDetector::new(run_id, VAD))),
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
