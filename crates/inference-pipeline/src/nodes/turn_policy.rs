//! The turn policy: a node of role `policy` with one input port of speech activity, as the
//! [`VoiceActivityDetector`](super::vad::VoiceActivityDetector) sends it, and one output port of
//! Control modality carrying the turn events of `inference.stream`. It is the single authority for
//! the turn commands of its pipeline (`docs/design/inference.md` 699): no other node opens or
//! completes a turn.
//!
//! At speech onset with no turn Open it runs `inference.stream.OpenTurn` in its run; once speech
//! ends, the detector's hold time being the endpoint silence, it runs
//! `inference.stream.CompleteTurn` on the turn it opened. The commands are answered by the store of
//! turns the node was constructed with, a [`TurnCommands`], and every answer is recorded on the
//! [`TurnPolicyHandle`]. An opened or completed turn is sent on as its `TurnOpened` or
//! `TurnCompleted`, in an envelope of the event's declared name that carries the timebase and
//! timestamp of the speech activity it answers and names that activity as its cause. A completion
//! the store refuses is the command's `wrong-state` outcome: it is recorded, nothing is sent on,
//! and the turn is no longer the node's. An `OpenTurn` the store refuses, its `unknown-run` or
//! `run-not-active` outcome, as for a run that is Draining, is recorded too and nothing is sent
//! on; the next speech onset asks again. Completing a turn moves none of its spans (606).
//!
//! On the end of input, a control event of kind `stop`, or once its input edge is closed, as on a
//! drain, it completes the turn it holds Open, sending its `TurnCompleted` on the wall clock with
//! no cause, as no speech activity answers it; on a `stop` it then passes the end of input on.
//! Then it ends. Cancelled, it sends nothing more. Content other than speech activity fails the
//! node.

use std::fmt;
use std::future;
use std::sync::Arc;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope};
use inference_core::id::{NodeId, RunId};
use inference_text::{
    OpenTurn, TurnCompleted, TurnId, TurnNotFound, TurnOpened, TurnStateConflict,
};

use super::vad::SpeechActivity;
use super::{Emitter, Next, Task, Watched, block_on, next, one_in_one_out};
use crate::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation};

/// An event of `inference.stream.Turn`'s lifecycle, as its command emitted it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEvent {
    /// `inference.stream.TurnOpened`.
    Opened(TurnOpened),
    /// `inference.stream.TurnCompleted`.
    Completed(TurnCompleted),
}

impl TurnEvent {
    /// The event's declared name, the `kind` of the envelope that carries it, such as
    /// `inference.stream.TurnCompleted`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Opened(_) => "inference.stream.TurnOpened",
            Self::Completed(_) => "inference.stream.TurnCompleted",
        }
    }
}

/// The payload of a run whose edges carry speech activity and turn events: the turn policy reads
/// a [`SpeechActivity`] out of it and builds a [`TurnEvent`], or a control event, into it.
pub trait TurnPayload: Send + Sized + 'static {
    /// The speech activity the payload carries.
    ///
    /// # Errors
    ///
    /// The payload, unchanged, when it carries none.
    fn into_activity(self) -> Result<SpeechActivity, Self>;

    /// The payload carrying `event`.
    fn from_turn(event: TurnEvent) -> Self;

    /// The payload of the control event `kind`; its envelope's `kind` is [`ControlKind::as_str`].
    fn control(kind: ControlKind) -> Self;
}

/// Why the store moved no turn on `inference.stream.CompleteTurn`: both are the command's
/// `wrong-state` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteTurnRefused {
    /// No stored turn carries the identity sent.
    Unknown(TurnNotFound),
    /// The turn is not Open; `current` is the state it holds.
    Conflict(TurnStateConflict),
}

impl fmt::Display for CompleteTurnRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(error) => fmt::Display::fmt(error, f),
            Self::Conflict(error) => fmt::Display::fmt(error, f),
        }
    }
}

/// The store of turns that answers the commands of a [`TurnPolicy`].
pub trait TurnCommands: Send + 'static {
    /// Runs `inference.stream.OpenTurn`.
    ///
    /// # Errors
    ///
    /// The refusal, described: the run is unknown or not Active.
    fn open_turn(&mut self, input: OpenTurn) -> Result<TurnOpened, String>;

    /// Runs `inference.stream.CompleteTurn` on the turn `turn_id`.
    ///
    /// # Errors
    ///
    /// The `wrong-state` outcome.
    fn complete_turn(&mut self, turn_id: TurnId) -> Result<TurnCompleted, CompleteTurnRefused>;
}

/// The store's answer to one command of the turn policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// `OpenTurn/outcome/opened`: the turn is Open and its event was sent on.
    Opened(TurnOpened),
    /// `OpenTurn/outcome/unknown-run` or `OpenTurn/outcome/run-not-active`, as the store
    /// described it in `refused`: no turn was opened and nothing was sent on.
    NotOpened { refused: String },
    /// `CompleteTurn/outcome/completed`: the turn is Completed and its event was sent on.
    Completed(TurnCompleted),
    /// `CompleteTurn/outcome/wrong-state`: the turn `turn_id` was not completed and nothing was
    /// sent on.
    WrongState {
        turn_id: TurnId,
        refused: CompleteTurnRefused,
    },
}

/// What the turn policy was asked to do and what it has done.
#[derive(Debug, Default)]
struct PolicyState {
    outcomes: Vec<TurnOutcome>,
    cancelled: bool,
    failure: Option<String>,
}

impl fmt::Debug for Watched<PolicyState> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

/// The turn policy node; see the module documentation.
pub struct TurnPolicy {
    run_id: RunId,
    commands: Option<Box<dyn TurnCommands>>,
    node_id: Option<NodeId>,
    state: Arc<Watched<PolicyState>>,
    task: Task,
}

impl fmt::Debug for TurnPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TurnPolicy")
            .field("run_id", &self.run_id)
            .field("node_id", &self.node_id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl TurnPolicy {
    /// A turn policy of the run `run_id` whose commands `commands` answers.
    #[must_use]
    pub fn new(run_id: RunId, commands: impl TurnCommands) -> Self {
        Self {
            run_id,
            commands: Some(Box::new(commands)),
            node_id: None,
            state: Arc::new(Watched::new(PolicyState::default())),
            task: Task::new(),
        }
    }

    /// The host's side of the turn policy: the record of the store's answers.
    #[must_use]
    pub fn handle(&self) -> TurnPolicyHandle {
        TurnPolicyHandle {
            state: Arc::clone(&self.state),
        }
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.state
            .read(|state| state.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

/// The host's side of a [`TurnPolicy`]. Clones share one policy.
#[derive(Debug, Clone)]
pub struct TurnPolicyHandle {
    state: Arc<Watched<PolicyState>>,
}

impl TurnPolicyHandle {
    /// Every answer of the store to the policy's commands, in the order the policy issued them.
    #[must_use]
    pub fn outcomes(&self) -> Vec<TurnOutcome> {
        self.state.read(|state| state.outcomes.clone())
    }
}

/// One started exchange of a [`TurnPolicy`].
struct Exchanging<P> {
    run_id: RunId,
    commands: Box<dyn TurnCommands>,
    /// The turn the policy opened and has not completed.
    open: Option<TurnId>,
    state: Arc<Watched<PolicyState>>,
    input: Box<dyn InputEndpoint<P>>,
    output: Box<dyn OutputEndpoint<P>>,
    emitter: Emitter,
}

impl<P: TurnPayload> Exchanging<P> {
    /// Sends `envelope` on, unless the policy was cancelled: it then sends nothing more.
    async fn send(&self, envelope: EventEnvelope<P>) -> Result<(), String> {
        if self.state.read(|state| state.cancelled) {
            return Ok(());
        }
        self.output
            .send(envelope)
            .await
            .map_err(|refused| refused.to_string())
    }

    /// The command `activity` calls for, if any, run against the store and its event sent on,
    /// answering `cause`, the envelope of the activity.
    async fn activity(
        &mut self,
        cause: &EventEnvelope<()>,
        activity: SpeechActivity,
    ) -> Result<(), String> {
        let (outcome, event) = match (activity, self.open) {
            (SpeechActivity::Started(_), None) => {
                match self.commands.open_turn(OpenTurn {
                    run_id: self.run_id,
                }) {
                    Ok(opened) => {
                        self.open = Some(opened.turn_id);
                        (TurnOutcome::Opened(opened), Some(TurnEvent::Opened(opened)))
                    }
                    Err(refused) => (TurnOutcome::NotOpened { refused }, None),
                }
            }
            (SpeechActivity::Ended(_), Some(turn_id)) => self.complete(turn_id),
            (SpeechActivity::Started(_), Some(_)) | (SpeechActivity::Ended(_), None) => {
                return Ok(());
            }
        };
        self.record(outcome);
        let Some(event) = event else {
            return Ok(());
        };
        let mut envelope = self
            .emitter
            .envelope(event.name(), None, P::from_turn(event));
        envelope.timebase = cause.timebase;
        envelope.timestamp = cause.timestamp;
        envelope.causation_id = Some(cause.event_id);
        self.send(envelope).await
    }

    /// Runs `CompleteTurn` on `turn_id`, the turn the policy holds Open, which is no longer the policy's
    /// whatever the store answers; the answer, and the event to send on if it completed.
    fn complete(&mut self, turn_id: TurnId) -> (TurnOutcome, Option<TurnEvent>) {
        self.open = None;
        match self.commands.complete_turn(turn_id) {
            Ok(completed) => (
                TurnOutcome::Completed(completed),
                Some(TurnEvent::Completed(completed)),
            ),
            Err(refused) => (TurnOutcome::WrongState { turn_id, refused }, None),
        }
    }

    fn record(&self, outcome: TurnOutcome) {
        self.state.update(|state| state.outcomes.push(outcome));
    }

    /// The end of input, a `stop` or a closed input edge: completes the turn the policy holds
    /// Open, if any, and sends its `TurnCompleted` on, on the wall clock with no cause, as no
    /// speech activity answers it.
    async fn end_of_input(&mut self) -> Result<(), String> {
        let Some(turn_id) = self.open else {
            return Ok(());
        };
        let (outcome, event) = self.complete(turn_id);
        self.record(outcome);
        match event {
            Some(event) => {
                let envelope = self
                    .emitter
                    .envelope(event.name(), None, P::from_turn(event));
                self.send(envelope).await
            }
            None => Ok(()),
        }
    }

    async fn exchange(&mut self) -> Result<(), String> {
        loop {
            let state = Arc::clone(&self.state);
            let next = next(self.input.as_mut(), &state, |state| {
                state.cancelled.then_some(())
            })
            .await;
            match next {
                Next::Signal(()) => return Ok(()),
                Next::Closed => return self.end_of_input().await,
                Next::Envelope(envelope) => match ControlKind::of(&envelope.kind) {
                    Some(ControlKind::Stop) => {
                        self.end_of_input().await?;
                        let stop = ControlKind::Stop;
                        let end = self.emitter.envelope(stop.as_str(), None, P::control(stop));
                        return self.send(end).await;
                    }
                    Some(_) => {}
                    None => {
                        let (cause, payload) = split(envelope);
                        let activity = payload.into_activity().map_err(|_| {
                            format!(
                                "the turn policy takes speech activity; an event of kind {} \
                                 arrived",
                                cause.kind
                            )
                        })?;
                        self.activity(&cause, activity).await?;
                    }
                },
            }
        }
    }

    fn run(mut self) {
        let ended = block_on(self.exchange());
        self.state.update(|state| state.failure = ended.err());
    }
}

/// The envelope without its payload, and the payload.
fn split<P>(envelope: EventEnvelope<P>) -> (EventEnvelope<()>, P) {
    let EventEnvelope {
        schema_version,
        event_id,
        run_id,
        session_id,
        stream_id,
        sequence,
        producer,
        port,
        kind,
        generation_id,
        timestamp,
        timebase,
        causation_id,
        payload,
    } = envelope;
    let header = EventEnvelope {
        schema_version,
        event_id,
        run_id,
        session_id,
        stream_id,
        sequence,
        producer,
        port,
        kind,
        generation_id,
        timestamp,
        timebase,
        causation_id,
        payload: (),
    };
    (header, payload)
}

impl<P: TurnPayload> NodeImplementation<P> for TurnPolicy {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started =
            one_in_one_out(self.node_id, endpoints).and_then(|(node_id, input, output)| {
                let commands = self
                    .commands
                    .take()
                    .ok_or_else(|| NodeError::new("the turn policy was started already"))?;
                let exchanging = Exchanging {
                    run_id: self.run_id,
                    commands,
                    open: None,
                    state: Arc::clone(&self.state),
                    input: input.endpoint,
                    emitter: Emitter::new(self.run_id, node_id, output.port_id.to_string()),
                    output: output.endpoint,
                };
                self.task.spawn(move || exchanging.run());
                Ok(())
            });
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(async move {
            self.task.finished().await.map_err(NodeError::new)?;
            self.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.state.update(|state| state.cancelled = true);
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(async move {
            let _already_reported_by_drain = self.task.finished().await;
        })
    }
}
