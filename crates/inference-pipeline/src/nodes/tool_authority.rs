//! The tool authority: the one node of a pipeline that authorizes and rejects tool calls
//! (`docs/design/inference.md` §15.1, 699), a node of role `policy` with one input port for
//! proposed calls, one output port for authorized calls and, when it is constructed with one
//! ([`ToolAuthority::rejecting_on`]), an output port for rejected calls.
//!
//! The authority decides each call once. A proposal of a call it already holds or decided is
//! refused as `AuthorizeToolCall/outcome/wrong-state`, naming the state it holds the call in, and
//! is neither held again nor sent on. For any other proposal its input edge delivers, it first
//! enforces its iteration rule: the proposals of one generation past the rule's maximum are
//! rejected `exhausted`. On a feedback loop the rule is the one the pipeline declares for the
//! loop whose feedback edge arrives at the authority (529, 546), read from the planned pipeline
//! ([`ToolAuthority::bounded_by`]); an authority on no loop may be given one
//! ([`ToolAuthority::with_iteration_bound`]). It then rejects a call for a tool it was
//! not constructed with (`denied`), then validates the arguments against the tool's schema and
//! rejects a call that fails it (`schema_invalid`, 848), then runs the default policy: it
//! authorizes the call, passing the tool's `requires_confirmation` as `confirmation_required` and
//! `confirmed` false. A tool that requires confirmation answers `unconfirmed`: the call is held
//! Proposed until the host answers it through the [`AuthorityHandle`], the confirmation hook
//! (853, 1054). A confirmed call is authorized with `confirmed` true; a denied one is rejected
//! `denied`. An authorized call is sent on the output edge of authorized calls; a rejected one is
//! sent on the edge of rejected calls when the authority has one, so that the proposer learns its
//! proposal will have no result, and otherwise on no edge. Every decision is recorded on the
//! handle.
//!
//! The schema is read as a subset of JSON Schema, the language `ess/domains/tools.yaml` leaves
//! unmapped: `type`, `properties`, `required`, `additionalProperties`, `items` and `enum`, the
//! annotations `title`, `description`, `$schema`, `$id`, `$comment`, `default` and `examples`,
//! and the schemas `true` and `false`. `enum` compares numbers by value, as `integer` does. A
//! tool whose schema uses another keyword, or one of these in a form other than JSON Schema's,
//! fails the node's preparation rather than being validated in part.
//!
//! The authority keeps every call it decided in a record ([`AuthorityHandle::calls`], the
//! `inference.tools.ToolCalls` view of its calls) that an executor node answering to it shares
//! ([`ToolExecutorNode::answering_to`](super::ToolExecutorNode::answering_to)): the executor starts
//! and ends each call on that record, and the authority rejects on it, each move made under the
//! record's lock, so a call is either rejected before it starts or started before it is rejected,
//! never both.
//!
//! Interrupted: on an envelope of kind `inference.stream.GenerationSuperseded` or
//! `inference.stream.GenerationCancelled` naming a generation (`docs/design/inference.md`
//! 632-639), the authority withdraws every call of that generation it holds for confirmation, so
//! the hook can no longer answer it, and rejects it, and every call of it the record holds
//! Authorized, `superseded`, in the order they were proposed; a call already Started, Completed or
//! Failed is left as it is, since nothing is rolled back (856). It then passes the envelope on, on
//! its edge of authorized calls, so that an executor running a call of the generation signals its
//! cancellation. Every later proposal of the generation is rejected `superseded`.
//!
//! On the end of input, a control event of kind `stop`, or once its input edge is closed, the
//! node waits for the host to answer every call it holds, then passes the end of input on, on
//! each of its output edges, and ends. Cancelled, it abandons the calls it holds and sends nothing
//! more: the check and each poll of a send are made under the lock the cancel is taken under, so
//! a send is either complete before the cancel or not made.

use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::future;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Poll;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId};
use inference_core::id::{GenerationId, NodeId, RunId};
use inference_tools::{
    AuthorizeError, AuthorizeToolCall, ConfirmationRequired, Json, RejectionReason, Tool, ToolCall,
    ToolCallAuthorized, ToolCallId, ToolCallRejected, ToolCallState, ToolCallStateConflict, ToolId,
};

use super::{
    Emitter, Next, Task, ToolCallEvent, ToolCallUpdate, ToolPayload, Watched, block_on, next,
    one_in_one_out,
};
use crate::{
    Declaration, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeInput,
    NodeOutput, PipelineId, PlannedPipeline, PortId,
};

/// What the authority decided for one call: the outcome of `inference.tools.AuthorizeToolCall`
/// or `inference.tools.RejectToolCall`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityOutcome {
    /// `AuthorizeToolCall/outcome/authorized`: the call is Authorized and was sent on.
    Authorized(ToolCallAuthorized),
    /// `AuthorizeToolCall/outcome/unconfirmed`: the tool requires confirmation and none was
    /// given; the call is held Proposed for the confirmation hook.
    Unconfirmed(ConfirmationRequired),
    /// `RejectToolCall/outcome/rejected`: the call is Rejected and was not sent on.
    Rejected(ToolCallRejected),
    /// `AuthorizeToolCall/outcome/wrong-state`: the call was proposed again after the authority
    /// held or decided it; `current` is the state the authority holds it in. Nothing moved.
    WrongState(ToolCallStateConflict),
}

/// One decision of the authority: the call as the decision left it, and the outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub call: ToolCall,
    pub outcome: AuthorityOutcome,
}

/// The kinds of the envelopes that interrupt a generation: its `GenerationSuperseded` and its
/// `GenerationCancelled`.
const INTERRUPTIONS: [&str; 2] = [
    "inference.stream.GenerationSuperseded",
    "inference.stream.GenerationCancelled",
];

/// Whether an envelope of `kind` interrupts the generation it names.
pub(crate) fn interrupts(kind: &str) -> bool {
    INTERRUPTIONS.contains(&kind)
}

/// Sends `envelope` on `endpoint` unless `cancelled` holds of the state `watched` keeps, which
/// the node's cancel sets under the same lock: the check and each poll of the send are made
/// holding it, so the send is either made before the cancel or not at all, and a cancel taken
/// while the send waits for room ends the wait. Once cancelled, nothing is sent and the answer is
/// `Ok`.
pub(crate) async fn send_unless<S, P>(
    watched: &Watched<S>,
    cancelled: fn(&S) -> bool,
    endpoint: &dyn OutputEndpoint<P>,
    envelope: EventEnvelope<P>,
) -> Result<(), String> {
    let mut sending = endpoint.send(envelope);
    future::poll_fn(|cx| {
        let mut guard = watched.lock();
        if cancelled(&guard.0) {
            return Poll::Ready(Ok(()));
        }
        guard.1 = Some(cx.waker().clone());
        sending
            .as_mut()
            .poll(cx)
            .map(|sent| sent.map_err(|refused| refused.to_string()))
    })
    .await
}

/// The record of the calls an authority decided and an executor answering to it moved: each call
/// as its last command left it, in the order it was first recorded. Clones share one record.
#[derive(Debug, Clone, Default)]
pub(crate) struct CallRecord(Arc<Mutex<Vec<ToolCall>>>);

impl CallRecord {
    fn lock(&self) -> MutexGuard<'_, Vec<ToolCall>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The call carrying `call_id`, as the record holds it.
    pub(crate) fn get(&self, call_id: ToolCallId) -> Option<ToolCall> {
        self.lock()
            .iter()
            .find(|call| call.call_id() == call_id)
            .cloned()
    }

    /// Records `call` in place of the call of its identity, or after the others.
    pub(crate) fn put(&self, call: ToolCall) {
        let mut calls = self.lock();
        put(&mut calls, call);
    }

    /// Moves the call of `delivered`'s identity, as the record holds it or, when it holds none,
    /// as delivered, by `command`, under the record's lock: the moved call is recorded and
    /// answered with the command's event; a refusal answers the call as it was, and nothing is
    /// recorded.
    pub(crate) fn moved<T, E>(
        &self,
        delivered: ToolCall,
        command: impl FnOnce(&ToolCall) -> Result<(ToolCall, T), E>,
    ) -> Result<(ToolCall, T), (ToolCall, E)> {
        let mut calls = self.lock();
        let call = calls
            .iter()
            .find(|call| call.call_id() == delivered.call_id())
            .cloned()
            .unwrap_or(delivered);
        match command(&call) {
            Ok((moved, event)) => {
                put(&mut calls, moved.clone());
                Ok((moved, event))
            }
            Err(refused) => Err((call, refused)),
        }
    }

    /// Rejects every call of `generation_id` the record holds Proposed or Authorized with
    /// `reason`, under the record's lock, in the order the record holds them.
    fn reject_generation(
        &self,
        generation_id: GenerationId,
        reason: RejectionReason,
    ) -> Vec<(ToolCall, ToolCallRejected)> {
        let mut calls = self.lock();
        let mut rejected = Vec::new();
        for call in calls.iter_mut() {
            if call.generation_id() != generation_id {
                continue;
            }
            if let Ok((moved, event)) = call.reject(reason) {
                *call = moved.clone();
                rejected.push((moved, event));
            }
        }
        rejected
    }

    /// Every call, in the order it was first recorded.
    pub(crate) fn all(&self) -> Vec<ToolCall> {
        self.lock().clone()
    }
}

/// Puts `call` in place of the call of its identity among `calls`, or after them.
fn put(calls: &mut Vec<ToolCall>, call: ToolCall) {
    match calls
        .iter_mut()
        .find(|held| held.call_id() == call.call_id())
    {
        Some(held) => *held = call,
        None => calls.push(call),
    }
}

/// A call held for confirmation, with the envelope that proposed it.
#[derive(Debug, Clone)]
struct Held {
    call: ToolCall,
    cause: EventId,
}

/// What the authority holds and has done.
#[derive(Debug, Default)]
struct AuthorityState {
    held: Vec<Held>,
    /// The hook's answers not yet acted on: the call, and whether it was confirmed.
    answers: VecDeque<(ToolCallId, bool)>,
    decisions: Vec<Decision>,
    cancelled: bool,
    ended: bool,
    failure: Option<String>,
}

impl fmt::Debug for Watched<AuthorityState> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

/// What the exchange acts on besides its input.
enum Signal {
    Cancel,
    Answer(Held, bool),
    Settled,
}

impl AuthorityState {
    /// A cancel first, then an answered call, then, once the input ended, that nothing is held.
    fn signal(&mut self, input_ended: bool) -> Option<Signal> {
        if self.cancelled {
            return Some(Signal::Cancel);
        }
        if let Some((call_id, confirmed)) = self.answers.pop_front()
            && let Some(index) = self
                .held
                .iter()
                .position(|held| held.call.call_id() == call_id)
        {
            return Some(Signal::Answer(self.held.remove(index), confirmed));
        }
        (input_ended && self.held.is_empty()).then_some(Signal::Settled)
    }
}

/// No `inference.graph.FeedbackLoop` of the pipeline `pipeline_id` bounds an edge into the node
/// `node_id`, so there is no declared iteration rule for its authority to enforce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IterationRuleNotDeclared {
    pub pipeline_id: PipelineId,
    pub node_id: NodeId,
}

impl fmt::Display for IterationRuleNotDeclared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no feedback loop of pipeline {} bounds an edge into node {}",
            self.pipeline_id, self.node_id
        )
    }
}

impl Error for IterationRuleNotDeclared {}

/// The tool authority node; see the module documentation.
#[derive(Debug)]
pub struct ToolAuthority {
    run_id: RunId,
    tools: Arc<HashMap<ToolId, Tool>>,
    max_iterations: Option<NonZeroUsize>,
    /// The iteration rule the pipeline declares for the authority, once it was read.
    declared: Option<NonZeroUsize>,
    rejections: Option<PortId>,
    node_id: Option<NodeId>,
    state: Arc<Watched<AuthorityState>>,
    calls: CallRecord,
    task: Task,
}

impl ToolAuthority {
    /// An authority of the run `run_id` deciding calls for `tools`, the declared tools it knows.
    #[must_use]
    pub fn new(run_id: RunId, tools: impl IntoIterator<Item = Tool>) -> Self {
        Self {
            run_id,
            tools: Arc::new(
                tools
                    .into_iter()
                    .map(|tool| (tool.tool_id(), tool))
                    .collect(),
            ),
            max_iterations: None,
            declared: None,
            rejections: None,
            node_id: None,
            state: Arc::new(Watched::new(AuthorityState::default())),
            calls: CallRecord::default(),
            task: Task::new(),
        }
    }

    /// The same authority enforcing an iteration rule of `max_iterations`: of the proposals of
    /// one generation it decides, the first `max_iterations` are decided as before and every later
    /// one is rejected `exhausted`. An authority on a feedback loop takes its rule from the loop's
    /// declaration instead ([`ToolAuthority::bounded_by`]); one given both refuses to prepare
    /// unless they agree.
    #[must_use]
    pub const fn with_iteration_bound(mut self, max_iterations: NonZeroUsize) -> Self {
        self.max_iterations = Some(max_iterations);
        self
    }

    /// The same authority enforcing the iteration rule `planned` declares for it: the
    /// `max_iterations` of the `inference.graph.FeedbackLoop` of that pipeline, held in
    /// `declaration`, whose feedback edge arrives at a port of the node `node_id`, the authority's
    /// node. Of several such loops, the smallest maximum holds. A rule given afterwards through
    /// [`ToolAuthority::with_iteration_bound`] that differs from it makes the node refuse to
    /// prepare, so the authority enforces the declared rule or none.
    ///
    /// # Errors
    ///
    /// [`IterationRuleNotDeclared`] when no feedback loop of the pipeline bounds an edge into the
    /// node; the authority is not returned.
    pub fn bounded_by(
        self,
        declaration: &Declaration,
        planned: &PlannedPipeline,
        node_id: NodeId,
    ) -> Result<Self, IterationRuleNotDeclared> {
        let pipeline_id = planned.pipeline_id();
        let into_node = |edge_id| {
            declaration.edges().any(|edge| {
                edge.edge_id() == edge_id
                    && edge.pipeline_id() == pipeline_id
                    && declaration
                        .port(edge.to_port())
                        .is_some_and(|port| port.node_id() == node_id)
            })
        };
        let declared = declaration
            .feedback_loops()
            .filter(|feedback| {
                feedback.pipeline_id() == pipeline_id && into_node(feedback.edge_id())
            })
            .map(crate::FeedbackLoop::max_iterations)
            .min()
            .ok_or(IterationRuleNotDeclared {
                pipeline_id,
                node_id,
            })?;
        Ok(Self {
            max_iterations: Some(declared),
            declared: Some(declared),
            ..self
        })
    }

    /// The same authority sending every rejected call on the output edge that leaves its port
    /// `port_id`, and its authorized calls on its other output edge; it is then handed exactly
    /// those two output edges.
    #[must_use]
    pub const fn rejecting_on(mut self, port_id: PortId) -> Self {
        self.rejections = Some(port_id);
        self
    }

    /// The host's side of the authority: the confirmation hook and the record of its decisions.
    #[must_use]
    pub fn handle(&self) -> AuthorityHandle {
        AuthorityHandle {
            state: Arc::clone(&self.state),
            calls: self.calls.clone(),
        }
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.state
            .read(|state| state.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

/// The host's side of a [`ToolAuthority`]: the confirmation hook, which confirms or denies a call
/// held for confirmation, and the record of every decision. Clones share one authority.
#[derive(Debug, Clone)]
pub struct AuthorityHandle {
    state: Arc<Watched<AuthorityState>>,
    calls: CallRecord,
}

impl AuthorityHandle {
    /// Every call held Proposed for confirmation, in the order it was held.
    #[must_use]
    pub fn held(&self) -> Vec<ToolCall> {
        self.state
            .read(|state| state.held.iter().map(|held| held.call.clone()).collect())
    }

    /// Confirms the held call `call_id`: the authority authorizes it with `confirmed` true. False,
    /// confirming nothing, when no call of that identity is held, when it was answered already,
    /// or once the authority was cancelled or ended.
    #[must_use]
    pub fn confirm(&self, call_id: ToolCallId) -> bool {
        self.answer(call_id, true)
    }

    /// Denies the held call `call_id`: the authority rejects it `denied`. False, denying nothing,
    /// as [`AuthorityHandle::confirm`] answers false.
    #[must_use]
    pub fn deny(&self, call_id: ToolCallId) -> bool {
        self.answer(call_id, false)
    }

    fn answer(&self, call_id: ToolCallId, confirmed: bool) -> bool {
        self.state.update(|state| {
            let held = state.held.iter().any(|held| held.call.call_id() == call_id);
            let answered = state
                .answers
                .iter()
                .any(|(answered, _)| *answered == call_id);
            if state.cancelled || state.ended || !held || answered {
                return false;
            }
            state.answers.push_back((call_id, confirmed));
            true
        })
    }

    /// Every decision of the authority, in the order it made them.
    #[must_use]
    pub fn decisions(&self) -> Vec<Decision> {
        self.state.read(|state| state.decisions.clone())
    }

    /// Every call the authority decided, as the last command of the authority or of an executor
    /// answering to it left it, in the order the authority first decided them: the rows of the
    /// `inference.tools.ToolCalls` view of its calls.
    #[must_use]
    pub fn calls(&self) -> Vec<ToolCall> {
        self.calls.all()
    }

    /// The record the authority keeps its calls in, for an executor answering to it.
    pub(crate) fn record(&self) -> CallRecord {
        self.calls.clone()
    }
}

/// One started exchange of a [`ToolAuthority`].
struct Exchanging<P> {
    tools: Arc<HashMap<ToolId, Tool>>,
    max_iterations: Option<NonZeroUsize>,
    state: Arc<Watched<AuthorityState>>,
    /// Every call the authority held or decided, as the last command left it.
    calls: CallRecord,
    /// The generations an interruption named.
    interrupted: HashSet<GenerationId>,
    /// How many proposals of each generation the authority decided.
    iterations: HashMap<GenerationId, usize>,
    input: Box<dyn InputEndpoint<P>>,
    output: Sending<P>,
    /// The edge of rejected calls, when the authority has one.
    rejected: Option<Sending<P>>,
}

/// One output edge of the authority and the stream it emits on it.
struct Sending<P> {
    endpoint: Box<dyn OutputEndpoint<P>>,
    emitter: Emitter,
}

/// Which output edge an envelope leaves on.
#[derive(Clone, Copy)]
enum OutputEdge {
    Authorized,
    Rejected,
}

impl<P: ToolPayload> Exchanging<P> {
    /// Sends `envelope` on `edge`, unless the authority was cancelled: it then sends nothing more
    /// ([`send_unless`]).
    async fn send(&self, edge: OutputEdge, envelope: EventEnvelope<P>) -> Result<(), String> {
        let sending = match edge {
            OutputEdge::Authorized => &self.output,
            OutputEdge::Rejected => match &self.rejected {
                Some(sending) => sending,
                None => return Ok(()),
            },
        };
        send_unless(
            &self.state,
            |state| state.cancelled,
            sending.endpoint.as_ref(),
            envelope,
        )
        .await
    }

    /// Acts on `envelope`, which interrupts the generation it names; see the module
    /// documentation.
    async fn interruption(&mut self, envelope: EventEnvelope<P>) -> Result<(), String> {
        let generation_id = envelope.generation_id.ok_or_else(|| {
            format!(
                "an envelope of kind {} names no generation to interrupt",
                envelope.kind
            )
        })?;
        self.interrupted.insert(generation_id);
        self.state.update(|state| {
            state
                .held
                .retain(|held| held.call.generation_id() != generation_id);
        });
        let rejected = self
            .calls
            .reject_generation(generation_id, RejectionReason::Superseded);
        for (call, event) in rejected {
            let decision = Decision {
                call,
                outcome: AuthorityOutcome::Rejected(event),
            };
            self.decided(decision, envelope.event_id).await?;
        }
        let mut forwarded = self
            .output
            .emitter
            .envelope(&envelope.kind, None, envelope.payload);
        forwarded.generation_id = Some(generation_id);
        forwarded.causation_id = Some(envelope.event_id);
        self.send(OutputEdge::Authorized, forwarded).await
    }

    /// The end of input, on each output edge.
    async fn end(&mut self) -> Result<(), String> {
        let end = self.output.emitter.control(ControlKind::Stop);
        self.send(OutputEdge::Authorized, end).await?;
        if let Some(rejected) = self.rejected.as_mut() {
            let end = rejected.emitter.control(ControlKind::Stop);
            self.send(OutputEdge::Rejected, end).await?;
        }
        Ok(())
    }

    /// Records `decision` and sends the authorized call on, or the rejected one back, answering
    /// `cause`.
    async fn decided(&mut self, decision: Decision, cause: EventId) -> Result<(), String> {
        let sent = match &decision.outcome {
            AuthorityOutcome::Authorized(event) => Some((
                OutputEdge::Authorized,
                ToolCallUpdate {
                    call: decision.call.clone(),
                    event: ToolCallEvent::Authorized(*event),
                },
            )),
            AuthorityOutcome::Rejected(event) => Some((
                OutputEdge::Rejected,
                ToolCallUpdate {
                    call: decision.call.clone(),
                    event: ToolCallEvent::Rejected(*event),
                },
            )),
            AuthorityOutcome::Unconfirmed(_) => {
                let held = Held {
                    call: decision.call.clone(),
                    cause,
                };
                self.state.update(|state| state.held.push(held));
                None
            }
            AuthorityOutcome::WrongState(_) => None,
        };
        if !matches!(decision.outcome, AuthorityOutcome::WrongState(_)) {
            self.calls.put(decision.call.clone());
        }
        self.state.update(|state| state.decisions.push(decision));
        match sent {
            Some((OutputEdge::Authorized, update)) => {
                let envelope = self.output.emitter.update(update, cause);
                self.send(OutputEdge::Authorized, envelope).await
            }
            Some((OutputEdge::Rejected, update)) => match self.rejected.as_mut() {
                Some(rejected) => {
                    let envelope = rejected.emitter.update(update, cause);
                    self.send(OutputEdge::Rejected, envelope).await
                }
                None => Ok(()),
            },
            None => Ok(()),
        }
    }

    /// Rejects `call` with `reason`.
    fn reject(call: &ToolCall, reason: RejectionReason) -> Result<Decision, String> {
        let (call, event) = call.reject(reason).map_err(|refused| refused.to_string())?;
        Ok(Decision {
            call,
            outcome: AuthorityOutcome::Rejected(event),
        })
    }

    /// Authorizes `call` for `tool`, with the hook's answer `confirmed`.
    fn authorize(call: &ToolCall, tool: &Tool, confirmed: bool) -> Result<Decision, String> {
        let input = AuthorizeToolCall {
            call_id: call.call_id(),
            confirmation_required: tool.requires_confirmation(),
            confirmed,
        };
        match call.authorize(&input) {
            Ok((call, event)) => Ok(Decision {
                call,
                outcome: AuthorityOutcome::Authorized(event),
            }),
            Err(AuthorizeError::Unconfirmed(refusal)) => Ok(Decision {
                call: call.clone(),
                outcome: AuthorityOutcome::Unconfirmed(refusal),
            }),
            Err(AuthorizeError::Conflict(conflict)) => Err(conflict.to_string()),
        }
    }

    /// The decision for the proposed `call`: a call held or decided already, then a generation
    /// an interruption named, then the iteration rule, then the tool, then the schema, then the
    /// policy.
    fn decide(&mut self, call: &ToolCall) -> Result<Decision, String> {
        if let Some(known) = self.calls.get(call.call_id()) {
            return Ok(Decision {
                outcome: AuthorityOutcome::WrongState(ToolCallStateConflict {
                    current: known.state(),
                }),
                call: known,
            });
        }
        if self.interrupted.contains(&call.generation_id()) {
            return Self::reject(call, RejectionReason::Superseded);
        }
        let iteration = self.iterations.entry(call.generation_id()).or_default();
        *iteration += 1;
        if self
            .max_iterations
            .is_some_and(|max_iterations| *iteration > max_iterations.get())
        {
            return Self::reject(call, RejectionReason::Exhausted);
        }
        let Some(tool) = self.tools.get(&call.tool_id()) else {
            return Self::reject(call, RejectionReason::Denied);
        };
        if !admits(tool.schema(), call.arguments()) {
            return Self::reject(call, RejectionReason::SchemaInvalid);
        }
        Self::authorize(call, tool, false)
    }

    /// The decision for the content of an envelope: a proposed call, or a failure naming what
    /// arrived instead.
    async fn proposal(&mut self, payload: P, kind: &str, cause: EventId) -> Result<(), String> {
        let call = match payload.into_tool_call() {
            Ok(update) if update.call.state() == ToolCallState::Proposed => update.call,
            Ok(update) => {
                return Err(format!(
                    "the tool authority takes proposed calls; call {} arrived {:?}",
                    update.call.call_id(),
                    update.call.state()
                ));
            }
            Err(_) => {
                return Err(format!(
                    "the tool authority takes proposed calls; an event of kind {kind} arrived"
                ));
            }
        };
        let decision = self.decide(&call)?;
        self.decided(decision, cause).await
    }

    /// The decision for the held call `held`, once the hook answered `confirmed`.
    fn answered(&self, held: &Held, confirmed: bool) -> Result<Decision, String> {
        if !confirmed {
            return Self::reject(&held.call, RejectionReason::Denied);
        }
        let tool = self
            .tools
            .get(&held.call.tool_id())
            .ok_or_else(|| format!("the tool of the held call {} is gone", held.call.call_id()))?;
        Self::authorize(&held.call, tool, true)
    }

    async fn exchange(&mut self) -> Result<(), String> {
        let mut input_ended = false;
        let mut end_of_input = false;
        loop {
            let next = if input_ended {
                let state = Arc::clone(&self.state);
                Next::Signal(state.until(|state| state.signal(true)).await)
            } else {
                let state = Arc::clone(&self.state);
                next(self.input.as_mut(), &state, |state| state.signal(false)).await
            };
            match next {
                Next::Signal(Signal::Cancel) => return Ok(()),
                Next::Signal(Signal::Answer(held, confirmed)) => {
                    let decision = self.answered(&held, confirmed)?;
                    self.decided(decision, held.cause).await?;
                }
                Next::Signal(Signal::Settled) => {
                    if end_of_input {
                        self.end().await?;
                    }
                    return Ok(());
                }
                Next::Envelope(envelope) => match ControlKind::of(&envelope.kind) {
                    Some(ControlKind::Stop) => {
                        input_ended = true;
                        end_of_input = true;
                    }
                    Some(_) => {}
                    None if interrupts(&envelope.kind) => self.interruption(envelope).await?,
                    None => {
                        self.proposal(envelope.payload, &envelope.kind, envelope.event_id)
                            .await?;
                    }
                },
                Next::Closed => input_ended = true,
            }
        }
    }

    fn run(mut self) {
        let ended = block_on(self.exchange());
        self.state.update(|state| {
            state.ended = true;
            state.failure = ended.err();
        });
    }
}

impl<P: ToolPayload> NodeImplementation<P> for ToolAuthority {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        if let Some(declared) = self.declared
            && self.max_iterations != Some(declared)
        {
            return Box::pin(future::ready(Err(NodeError::new(format!(
                "the tool authority was given the iteration rule {:?}, and its pipeline declares \
                 {declared}",
                self.max_iterations.map(NonZeroUsize::get)
            )))));
        }
        let supported = self.tools.values().try_for_each(|tool| {
            supported(tool.schema()).map_err(|unsupported| {
                NodeError::new(format!(
                    "the schema of tool {} ({}) uses {unsupported}, which the tool authority does \
                     not validate",
                    tool.name(),
                    tool.tool_id()
                ))
            })
        });
        Box::pin(future::ready(supported))
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = shaped(self.node_id, self.rejections, endpoints).map(
            |(node_id, input, output, rejected)| {
                let sending = |output: NodeOutput<P>| Sending {
                    emitter: Emitter::new(self.run_id, node_id, output.port_id.to_string()),
                    endpoint: output.endpoint,
                };
                let exchanging = Exchanging {
                    tools: Arc::clone(&self.tools),
                    max_iterations: self.max_iterations,
                    state: Arc::clone(&self.state),
                    calls: self.calls.clone(),
                    interrupted: HashSet::new(),
                    iterations: HashMap::new(),
                    input: input.endpoint,
                    output: sending(output),
                    rejected: rejected.map(sending),
                };
                self.task.spawn(move || exchanging.run());
            },
        );
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

/// What a started authority takes: its identity, its one input edge, its edge of authorized calls
/// and its edge of rejected calls, if it has one.
type Shaped<P> = (NodeId, NodeInput<P>, NodeOutput<P>, Option<NodeOutput<P>>);

/// The node's identity, its one input edge, its edge of authorized calls and, when it rejects on
/// the port `rejections`, its edge of rejected calls, once it was described and handed exactly
/// those; otherwise the error saying what it was handed instead.
fn shaped<P>(
    node_id: Option<NodeId>,
    rejections: Option<PortId>,
    mut endpoints: NodeEndpoints<P>,
) -> Result<Shaped<P>, NodeError> {
    let Some(port_id) = rejections else {
        return one_in_one_out(node_id, endpoints)
            .map(|(node_id, input, output)| (node_id, input, output, None));
    };
    let rejected = endpoints
        .outputs
        .iter()
        .position(|output| output.port_id == port_id);
    match rejected {
        Some(index) if endpoints.inputs.len() == 1 && endpoints.outputs.len() == 2 => {
            let rejected = endpoints.outputs.remove(index);
            one_in_one_out(node_id, endpoints)
                .map(|(node_id, input, output)| (node_id, input, output, Some(rejected)))
        }
        _ => Err(NodeError::new(format!(
            "the node takes 1 input edge and 2 output edges, one of them from its port {port_id}, \
             and was handed {} and {}",
            endpoints.inputs.len(),
            endpoints.outputs.len()
        ))),
    }
}

/// The JSON Schema type names [`typed`] reads.
const TYPES: [&str; 7] = [
    "object", "array", "string", "boolean", "null", "number", "integer",
];

/// The annotations a schema may carry, which validate nothing.
const ANNOTATIONS: [&str; 7] = [
    "title",
    "description",
    "$schema",
    "$id",
    "$comment",
    "default",
    "examples",
];

/// Nothing, when `schema` and every schema inside it uses only the keywords [`admits`]
/// validates, each in JSON Schema's form, and the [`ANNOTATIONS`]; otherwise what it uses
/// instead.
fn supported(schema: &Json) -> Result<(), String> {
    let keywords = match schema {
        Json::Bool(_) => return Ok(()),
        Json::Object(keywords) => keywords,
        other => return Err(format!("{other} as a schema")),
    };
    for (keyword, value) in keywords {
        let well_formed = match keyword.as_str() {
            "type" => match value {
                Json::String(name) => TYPES.contains(&name.as_str()),
                Json::Array(names) => names
                    .iter()
                    .all(|name| name.as_str().is_some_and(|name| TYPES.contains(&name))),
                _ => false,
            },
            "properties" => match value {
                Json::Object(properties) => {
                    properties.values().try_for_each(supported)?;
                    true
                }
                _ => false,
            },
            "required" => value
                .as_array()
                .is_some_and(|names| names.iter().all(Json::is_string)),
            "additionalProperties" | "items" => {
                supported(value)?;
                true
            }
            "enum" => value.is_array(),
            annotation if ANNOTATIONS.contains(&annotation) => true,
            other => return Err(format!("the keyword {other:?}")),
        };
        if !well_formed {
            return Err(format!("{keyword:?} in the form {value}"));
        }
    }
    Ok(())
}

/// Whether `value` is of the JSON Schema type `name`.
fn typed(name: &str, value: &Json) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|n| n.fract() == 0.0)
        }
        _ => false,
    }
}

/// Whether `a` and `b` are equal as JSON Schema compares instances: numbers by value, arrays
/// element by element, objects member by member.
fn equal(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => {
            if let (Some(x), Some(y)) = (x.as_i64(), y.as_i64()) {
                x == y
            } else if let (Some(x), Some(y)) = (x.as_u64(), y.as_u64()) {
                x == y
            } else {
                x.as_f64() == y.as_f64()
            }
        }
        (Json::Array(x), Json::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| equal(a, b))
        }
        (Json::Object(x), Json::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(name, a)| y.get(name).is_some_and(|b| equal(a, b)))
        }
        _ => a == b,
    }
}

/// Whether `value` satisfies `schema`, read as the subset of JSON Schema [`supported`] admits.
fn admits(schema: &Json, value: &Json) -> bool {
    let Json::Object(keywords) = schema else {
        return schema.as_bool().unwrap_or(false);
    };
    let type_holds = match keywords.get("type") {
        None => true,
        Some(Json::String(name)) => typed(name, value),
        Some(Json::Array(names)) => names
            .iter()
            .any(|name| name.as_str().is_some_and(|name| typed(name, value))),
        Some(_) => false,
    };
    let enum_holds = match keywords.get("enum") {
        None => true,
        Some(Json::Array(allowed)) => allowed.iter().any(|member| equal(member, value)),
        Some(_) => false,
    };
    if !type_holds || !enum_holds {
        return false;
    }
    if let Json::Object(members) = value {
        let properties = keywords.get("properties").and_then(Json::as_object);
        let required = keywords
            .get("required")
            .and_then(Json::as_array)
            .map_or(&[][..], Vec::as_slice);
        if !required
            .iter()
            .all(|name| name.as_str().is_some_and(|name| members.contains_key(name)))
        {
            return false;
        }
        let additional = keywords.get("additionalProperties");
        let members_hold = members.iter().all(|(name, member)| {
            match properties.and_then(|properties| properties.get(name)) {
                Some(schema) => admits(schema, member),
                None => additional.is_none_or(|schema| admits(schema, member)),
            }
        });
        if !members_hold {
            return false;
        }
    }
    if let (Json::Array(elements), Some(items)) = (value, keywords.get("items")) {
        return elements.iter().all(|element| admits(items, element));
    }
    true
}

#[cfg(test)]
mod tests {
    use std::future::{self, Future};
    use std::pin::pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};

    use inference_core::endpoint::{EndpointFuture, OutputEndpoint, SendError};
    use inference_core::envelope::{EventEnvelope, EventId, Timebase};
    use inference_core::id::{NodeId, RunId, StreamId};
    use serde_json::json;

    use super::super::Watched;
    use super::{admits, send_unless, supported};

    /// An edge that takes an envelope on the second poll of its send: the first finds it full.
    #[derive(Default)]
    struct FullOnce {
        taken: Mutex<Vec<u8>>,
    }

    impl OutputEndpoint<u8> for FullOnce {
        fn send(
            &self,
            envelope: EventEnvelope<u8>,
        ) -> EndpointFuture<'_, Result<(), SendError<u8>>> {
            let mut waited = false;
            Box::pin(future::poll_fn(move |cx| {
                if !waited {
                    waited = true;
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                self.taken
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(envelope.payload);
                Poll::Ready(Ok(()))
            }))
        }
    }

    fn envelope(payload: u8) -> EventEnvelope<u8> {
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: RunId::generate(),
            session_id: None,
            stream_id: StreamId::generate(),
            sequence: 1,
            producer: NodeId::generate(),
            port: "out".to_owned(),
            kind: "x".to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }

    #[test]
    fn a_cancel_taken_while_a_send_waits_ends_it_and_nothing_reaches_the_edge() {
        let edge = FullOnce::default();
        let cancelled = Watched::new(false);
        let mut context = Context::from_waker(Waker::noop());
        let mut sending = pin!(send_unless(
            &cancelled,
            |cancelled| *cancelled,
            &edge,
            envelope(1)
        ));
        assert!(sending.as_mut().poll(&mut context).is_pending());
        cancelled.update(|cancelled| *cancelled = true);
        assert_eq!(sending.as_mut().poll(&mut context), Poll::Ready(Ok(())));
        assert!(
            edge.taken.lock().expect("unpoisoned").is_empty(),
            "nothing after the cancel"
        );

        let mut sent = pin!(send_unless(
            &cancelled,
            |cancelled| *cancelled,
            &edge,
            envelope(2)
        ));
        assert_eq!(sent.as_mut().poll(&mut context), Poll::Ready(Ok(())));
        assert!(
            edge.taken.lock().expect("unpoisoned").is_empty(),
            "nor once cancelled"
        );

        let open = Watched::new(false);
        let mut taken = pin!(send_unless(
            &open,
            |cancelled| *cancelled,
            &edge,
            envelope(3)
        ));
        assert!(taken.as_mut().poll(&mut context).is_pending());
        assert_eq!(taken.as_mut().poll(&mut context), Poll::Ready(Ok(())));
        assert_eq!(*edge.taken.lock().expect("unpoisoned"), [3]);
    }

    #[test]
    fn the_schema_subset_admits_what_json_schema_admits_and_refuses_what_it_refuses() {
        let schema = json!({
            "type": "object",
            "description": "a city and how many days",
            "properties": {
                "city": {"type": "string"},
                "days": {"type": "integer"},
                "unit": {"enum": ["c", "f"]},
                "level": {"enum": [1, [2.0], {"n": 3}]},
                "tags": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["city"],
            "additionalProperties": false
        });
        assert_eq!(supported(&schema), Ok(()));
        for admitted in [
            json!({"city": "Berlin"}),
            json!({"city": "Berlin", "days": 3, "unit": "c", "tags": ["a"]}),
            json!({"city": "Berlin", "days": 3.0}),
            json!({"city": "Berlin", "level": 1.0}),
            json!({"city": "Berlin", "level": [2]}),
            json!({"city": "Berlin", "level": {"n": 3.0}}),
        ] {
            assert!(admits(&schema, &admitted), "{admitted}");
        }
        for refused in [
            json!({}),
            json!({"city": 7}),
            json!({"city": "Berlin", "days": 1.5}),
            json!({"city": "Berlin", "unit": "k"}),
            json!({"city": "Berlin", "level": 1.5}),
            json!({"city": "Berlin", "level": "1"}),
            json!({"city": "Berlin", "level": [2, 2]}),
            json!({"city": "Berlin", "tags": [1]}),
            json!({"city": "Berlin", "country": "DE"}),
            json!("Berlin"),
        ] {
            assert!(!admits(&schema, &refused), "{refused}");
        }
        assert!(admits(&json!(true), &json!(1)));
        assert!(!admits(&json!(false), &json!(1)));
        assert!(admits(&json!({}), &json!({"anything": [1]})));
    }

    #[test]
    fn a_schema_keyword_outside_the_subset_is_named_rather_than_ignored() {
        assert_eq!(
            supported(&json!({"type": "string", "pattern": "^a"})),
            Err("the keyword \"pattern\"".to_owned())
        );
        assert_eq!(
            supported(&json!({"properties": {"n": {"minimum": 1}}})),
            Err("the keyword \"minimum\"".to_owned())
        );
        assert_eq!(
            supported(&json!({"items": {"oneOf": []}})),
            Err("the keyword \"oneOf\"".to_owned())
        );
        assert_eq!(supported(&json!(3)), Err("3 as a schema".to_owned()));
    }

    #[test]
    fn a_keyword_of_the_subset_in_another_form_than_json_schemas_is_named_rather_than_ignored() {
        for malformed in [
            json!({"required": "city"}),
            json!({"required": [1]}),
            json!({"properties": ["city"]}),
            json!({"properties": {"city": "string"}}),
            json!({"type": "text"}),
            json!({"type": ["string", 1]}),
            json!({"type": 3}),
            json!({"enum": "a"}),
            json!({"items": [{"type": "string"}]}),
            json!({"additionalProperties": 3}),
        ] {
            assert!(supported(&malformed).is_err(), "{malformed}");
        }
        for well_formed in [
            json!({"type": ["string", "null"], "required": [], "properties": {}}),
            json!({"items": true, "additionalProperties": {"type": "integer"}, "enum": []}),
        ] {
            assert_eq!(supported(&well_formed), Ok(()), "{well_formed}");
        }
    }
}
