//! The subgraph node (`docs/design/inference.md` 544, A-14 at 1150): a node of a parent run that
//! runs its child pipeline as a nested child run, so that the run boundary is the cancellation and
//! tracing boundary.
//!
//! A [`SubgraphNode`] is the implementation of a node declared with
//! `inference.graph.AddSubgraphNode`, whose ports planning matched one to one with its child
//! pipeline's exposures. It drives a [`RunController`] of the child from its own lifecycle:
//!
//! | step of the subgraph node | what the child run does |
//! |---|---|
//! | `describe`, `described` | nothing yet; the node keeps its declaration and its instance |
//! | `prepare` | created under the node's instance as its parent (`NestRun`), validated, prepared |
//! | `start` | activated; each edge endpoint of the node is bridged to the child exposure of its port's name, and the child's run loop runs |
//! | `drain` | `stop` is sent into each ingress still open and a drain is asked for; the step returns once the child run is terminal |
//! | `cancel` | cancelled |
//! | `stop` | returns once the child run is terminal and its queues are dropped |
//!
//! A child run that failed fails the subgraph node's drain, or its prepare or start, with a
//! [`NodeError`] carrying the child's reason, which names the child node; the parent run then fails
//! with a reason naming the subgraph node. A child run that did not complete when drained fails
//! the drain too.
//!
//! Tracing needs no library: every envelope inside the child carries the child run's identity, as
//! its nodes stamp it. An envelope crossing the boundary, in either direction, is sent on the other
//! side with that side's run identity, a new event identity, the subgraph node as its producer, the
//! port or exposure name it crosses to as its port, and the crossed envelope's event identity as
//! its `causation_id`; its stream, sequence, generation, kind and payload are kept, so a child's
//! generation epochs stay the child's. A run's parent is read from its controller
//! ([`RunController::parent`]).
//!
//! The child's run loop and the bridges run on a thread of the subgraph node's own, polled by an
//! executor of its own, as the testkit's nodes do, so no caller is committed to one. A child
//! failure while the parent is Active and sends the node nothing more is seen at the parent's
//! drain; the node does not fail the parent on its own.

use std::fmt;
use std::future::{self, Future};
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::Duration;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_pipeline::{
    Declaration, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeInstanceId,
    PlannedPipeline, PortId,
};

use crate::controller::{RunController, RunError};
use crate::drive::RunRequests;
use crate::queue::EpochLookup;
use crate::run::{NodeInstanceState, RunState};

/// The node implementations of a child run: called once per node of the child, with the child
/// run's identity, which the node stamps on what it emits, and the node's declaration.
pub type ChildImplementations<P> =
    Box<dyn FnMut(RunId, &Node) -> Box<dyn NodeImplementation<P>> + Send>;

/// What the child run had become when the subgraph node stopped: its identity, its parent, its
/// state, the state of each of its instances in the declaration order of the child's nodes, and
/// whether every queue of it, edges' and exposures', was dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildAtStop {
    pub run_id: RunId,
    pub parent: Option<NodeInstanceId>,
    pub state: RunState,
    pub instances: Vec<NodeInstanceState>,
    pub queues_dropped: bool,
}

#[derive(Debug, Default)]
struct Seen {
    child_run_id: Option<RunId>,
    at_stop: Option<ChildAtStop>,
}

/// The caller's side of a [`SubgraphNode`]: what it reads of the child run.
#[derive(Debug, Clone, Default)]
pub struct SubgraphHandle {
    seen: Arc<Mutex<Seen>>,
}

impl SubgraphHandle {
    fn seen(&self) -> MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The child run's identity, once the node prepared it.
    #[must_use]
    pub fn child_run_id(&self) -> Option<RunId> {
        self.seen().child_run_id
    }

    /// The child run as the subgraph node left it when its stop returned; none before.
    #[must_use]
    pub fn at_stop(&self) -> Option<ChildAtStop> {
        self.seen().at_stop.clone()
    }
}

/// What the bridges of a running child are told, shared with them.
#[derive(Default)]
struct Bridges {
    state: Mutex<(BridgeState, Vec<Waker>)>,
}

#[derive(Default)]
struct BridgeState {
    /// The subgraph node drains: each ingress still open sends `stop` and ends.
    drain: bool,
    /// The subgraph node is cancelled: every bridge ends, forwarding nothing more.
    cancel: bool,
    /// The child run is terminal: each ingress ends.
    child_ended: bool,
    /// The thread of the child has returned.
    done: bool,
}

impl Bridges {
    fn update(&self, change: impl FnOnce(&mut BridgeState)) {
        let wakers = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            change(&mut state.0);
            std::mem::take(&mut state.1)
        };
        wakers.into_iter().for_each(Waker::wake);
    }

    /// Reads the state, or registers the task of `cx` to be woken by its next change.
    fn read<T>(&self, cx: &Context<'_>, read: impl FnOnce(&BridgeState) -> Option<T>) -> Option<T> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let answer = read(&state.0);
        if answer.is_none() {
            state.1.push(cx.waker().clone());
        }
        answer
    }

    async fn done(&self) {
        future::poll_fn(|cx| match self.read(cx, |state| state.done.then_some(())) {
            Some(()) => Poll::Ready(()),
            None => Poll::Pending,
        })
        .await;
    }
}

/// What the child is at.
enum Child<P> {
    /// Not prepared yet, or prepared and refused.
    None,
    /// Prepared, not started.
    Prepared(RunController<P>),
    /// Started: its loop and bridges run on `thread`.
    Running {
        requests: RunRequests,
        bridges: Arc<Bridges>,
        thread: JoinHandle<(RunController<P>, Result<(), RunError>)>,
    },
    /// Terminal, or refused at a step.
    Ended(RunController<P>),
}

/// A node of a parent run that runs its child pipeline as a nested child run; see the module
/// documentation.
pub struct SubgraphNode<P> {
    run_id: RunId,
    declaration: Declaration,
    child: PlannedPipeline,
    epochs: Arc<dyn EpochLookup>,
    implement: ChildImplementations<P>,
    node: Option<Node>,
    instance_id: Option<NodeInstanceId>,
    state: Child<P>,
    handle: SubgraphHandle,
}

impl<P> fmt::Debug for SubgraphNode<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SubgraphNode")
            .field("run_id", &self.run_id)
            .field("child", &self.child.pipeline_id())
            .field("instance_id", &self.instance_id)
            .finish_non_exhaustive()
    }
}

impl<P: From<ControlKind> + Send + 'static> SubgraphNode<P> {
    /// The subgraph node of the parent run `run_id`, running the planned pipeline `child`, which
    /// `declaration` declares together with the parent's ports; the child's queues read epochs
    /// through `epochs`, and `implement` makes the implementation of each node of the child.
    #[must_use]
    pub fn new(
        run_id: RunId,
        declaration: &Declaration,
        child: &PlannedPipeline,
        epochs: impl EpochLookup + 'static,
        implement: ChildImplementations<P>,
    ) -> Self {
        Self {
            run_id,
            declaration: declaration.clone(),
            child: child.clone(),
            epochs: Arc::new(epochs),
            implement,
            node: None,
            instance_id: None,
            state: Child::None,
            handle: SubgraphHandle::default(),
        }
    }

    /// The caller's side of the node.
    #[must_use]
    pub fn handle(&self) -> SubgraphHandle {
        self.handle.clone()
    }

    /// The error the subgraph node answers for a child run that failed with `failure`, or that
    /// ended otherwise than completed.
    fn child_failed(child: &RunController<P>) -> NodeError {
        let run_id = child.run().run_id();
        match child.run().failure() {
            Some(failure) => match failure.node_id {
                Some(node_id) => NodeError::new(format!(
                    "the child run {run_id} failed at its node {node_id}: {}",
                    failure.detail
                )),
                None => {
                    NodeError::new(format!("the child run {run_id} failed: {}", failure.detail))
                }
            },
            None => NodeError::new(format!(
                "the child run {run_id} ended {:?}, not Completed",
                child.run().state()
            )),
        }
    }

    /// The error for a refused step of the child run.
    fn refused(child: &RunController<P>, error: &RunError) -> NodeError {
        match error {
            RunError::NodeFailed(_) => Self::child_failed(child),
            other => NodeError::new(format!(
                "the child run {} refused a step: {other}",
                child.run().run_id()
            )),
        }
    }

    async fn prepare_child(&mut self) -> Result<(), NodeError> {
        let node = self.node.as_ref().ok_or_else(|| {
            NodeError::new("the subgraph node was prepared before it was described")
        })?;
        if node.subgraph_id() != Some(self.child.pipeline_id()) {
            return Err(NodeError::new(format!(
                "the node {} does not stand for the pipeline {}",
                node.node_id(),
                self.child.pipeline_id()
            )));
        }
        let parent = self.instance_id.ok_or_else(|| {
            NodeError::new("the subgraph node was prepared before its instance was described")
        })?;
        let child_run = RunId::generate();
        let implement = &mut self.implement;
        let mut child = RunController::create_child(
            child_run,
            &self.child,
            &self.declaration,
            parent,
            Arc::clone(&self.epochs),
            |node| implement(child_run, node),
        )
        .map_err(|error| NodeError::new(format!("the child run was not created: {error}")))?;
        self.handle.seen().child_run_id = Some(child_run);
        child
            .validate()
            .map_err(|error| NodeError::new(format!("the child run did not validate: {error}")))?;
        let prepared = child.prepare().await;
        if let Err(error) = prepared {
            let refused = Self::refused(&child, &error);
            self.state = Child::Ended(child);
            return Err(refused);
        }
        self.state = Child::Prepared(child);
        Ok(())
    }

    async fn start_child(&mut self, endpoints: NodeEndpoints<P>) -> Result<(), NodeError> {
        let Child::Prepared(mut child) = std::mem::replace(&mut self.state, Child::None) else {
            return Err(NodeError::new(
                "the subgraph node was started without a prepared child run",
            ));
        };
        let node_id = self.node.as_ref().map(Node::node_id).ok_or_else(|| {
            NodeError::new("the subgraph node was started before it was described")
        })?;
        let mut ends = child.exposed();
        let name = |port_id: PortId| {
            self.declaration
                .port(port_id)
                .map(|port| port.name().to_owned())
                .unwrap_or_default()
        };
        let child_run = child.run().run_id();
        let bridges = Arc::new(Bridges::default());
        let mut tasks: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = Vec::new();
        for input in endpoints.inputs {
            let exposure = name(input.port_id);
            let Some(ingress) = ends.ingress(&exposure) else {
                return Err(NodeError::new(format!(
                    "the child has no ingress named `{exposure}` for the port {}",
                    input.port_id
                )));
            };
            tasks.push(Box::pin(ingress_bridge(
                input.endpoint,
                ingress,
                Arc::clone(&bridges),
                Crossing {
                    run_id: child_run,
                    producer: node_id,
                    port: exposure,
                },
            )));
        }
        for output in endpoints.outputs {
            let port = name(output.port_id);
            let Some(egress) = ends.egress(&port) else {
                return Err(NodeError::new(format!(
                    "the child has no egress named `{port}` for the port {}",
                    output.port_id
                )));
            };
            tasks.push(Box::pin(egress_bridge(
                egress,
                output.endpoint,
                Arc::clone(&bridges),
                Crossing {
                    run_id: self.run_id,
                    producer: node_id,
                    port,
                },
            )));
        }
        drop(ends);
        if let Err(error) = child.activate().await {
            let refused = Self::refused(&child, &error);
            self.state = Child::Ended(child);
            return Err(refused);
        }
        let requests = child.requests();
        let thread_bridges = Arc::clone(&bridges);
        let thread = thread::spawn(move || {
            let result = block_on(run_child(&mut child, tasks, &thread_bridges));
            thread_bridges.update(|state| state.done = true);
            (child, result)
        });
        self.state = Child::Running {
            requests,
            bridges,
            thread,
        };
        Ok(())
    }

    /// Waits for the running child's thread to return and keeps what it returned.
    async fn ended(&mut self) {
        let Child::Running { bridges, .. } = &self.state else {
            return;
        };
        let bridges = Arc::clone(bridges);
        bridges.done().await;
        if let Child::Running { thread, .. } = std::mem::replace(&mut self.state, Child::None) {
            self.state = match thread.join() {
                Ok((child, _answered)) => Child::Ended(child),
                Err(_) => Child::None,
            };
        }
    }

    async fn drain_child(&mut self) -> Result<(), NodeError> {
        if let Child::Running {
            requests, bridges, ..
        } = &self.state
        {
            bridges.update(|state| state.drain = true);
            requests.drain();
        }
        self.ended().await;
        match &self.state {
            Child::Ended(child) if child.run().state() == RunState::Completed => Ok(()),
            Child::Ended(child) => Err(Self::child_failed(child)),
            Child::None | Child::Prepared(_) | Child::Running { .. } => Err(NodeError::new(
                "the subgraph node was drained without a running child run",
            )),
        }
    }

    async fn cancel_child(&mut self) {
        match &mut self.state {
            Child::Running {
                requests, bridges, ..
            } => {
                bridges.update(|state| state.cancel = true);
                requests.cancel();
            }
            Child::Prepared(child) => {
                let _cancelled_or_already_terminal = child.cancel().await;
            }
            Child::None | Child::Ended(..) => {}
        }
    }

    async fn stop_child(&mut self) {
        self.ended().await;
        if let Child::Prepared(child) = &mut self.state {
            let _cancelled_or_already_terminal = child.cancel().await;
        }
        let controller = match &self.state {
            Child::Prepared(child) | Child::Ended(child) => child,
            Child::None | Child::Running { .. } => return,
        };
        let at_stop = ChildAtStop {
            run_id: controller.run().run_id(),
            parent: controller.parent(),
            state: controller.run().state(),
            instances: controller
                .instances()
                .map(crate::run::NodeInstance::state)
                .collect(),
            queues_dropped: controller.queues().iter().all(|queue| queue.is_dropped())
                && controller
                    .exposure_queues()
                    .iter()
                    .all(|queue| queue.is_dropped()),
        };
        self.handle.seen().at_stop = Some(at_stop);
    }
}

impl<P: From<ControlKind> + Send + 'static> NodeImplementation<P> for SubgraphNode<P> {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node = Some(node.clone());
        Box::pin(future::ready(()))
    }

    fn described(&mut self, instance_id: NodeInstanceId) {
        self.instance_id = Some(instance_id);
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(self.prepare_child())
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(self.start_child(endpoints))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(self.drain_child())
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(self.cancel_child())
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(self.stop_child())
    }
}

/// Where a crossing envelope is sent: the run it is stamped with, the subgraph node as producer,
/// and the port or exposure name it arrives through.
struct Crossing {
    run_id: RunId,
    producer: NodeId,
    port: String,
}

impl Crossing {
    /// `envelope` as sent on this side: a new event identity caused by the crossed one.
    fn cross<P>(&self, envelope: EventEnvelope<P>) -> EventEnvelope<P> {
        EventEnvelope {
            event_id: EventId::generate(),
            run_id: self.run_id,
            producer: self.producer,
            port: self.port.clone(),
            causation_id: Some(envelope.event_id),
            ..envelope
        }
    }

    /// The end of input, a control event of kind `stop`, on a stream of its own.
    fn stop<P: From<ControlKind>>(&self) -> EventEnvelope<P> {
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: StreamId::generate(),
            sequence: 1,
            producer: self.producer,
            port: self.port.clone(),
            kind: ControlKind::Stop.as_str().to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: P::from(ControlKind::Stop),
        }
    }
}

/// What a bridge waits on next.
enum Next<P> {
    Received(Option<EventEnvelope<P>>),
    Drain,
    End,
}

/// The next envelope of `input`: `first` answers from the bridges' state before anything is
/// received, then what `input` holds is taken, and `then` answers only while it holds nothing.
async fn next<P>(
    input: &mut dyn InputEndpoint<P>,
    bridges: &Bridges,
    first: impl Fn(&BridgeState) -> Option<Next<P>>,
    then: impl Fn(&BridgeState) -> Option<Next<P>>,
) -> Next<P> {
    let mut recv = input.recv();
    future::poll_fn(|cx| {
        if let Some(next) = bridges.read(cx, &first) {
            return Poll::Ready(next);
        }
        if let Poll::Ready(received) = recv.as_mut().poll(cx) {
            return Poll::Ready(Next::Received(received));
        }
        bridges.read(cx, &then).map_or(Poll::Pending, Poll::Ready)
    })
    .await
}

/// Sends `envelope` on `output`, again while the queue is full, until it takes it, refuses it
/// otherwise, or the bridges are cancelled.
async fn send<P>(
    output: &dyn OutputEndpoint<P>,
    envelope: EventEnvelope<P>,
    bridges: &Bridges,
) -> bool {
    let mut envelope = envelope;
    loop {
        match output.send(envelope).await {
            Ok(()) => return true,
            Err(SendError::Full(refused)) => {
                let cancelled = future::poll_fn(|cx| {
                    Poll::Ready(
                        bridges
                            .read(cx, |state| state.cancel.then_some(()))
                            .is_some(),
                    )
                })
                .await;
                if cancelled {
                    return false;
                }
                envelope = refused;
                thread::sleep(Duration::from_millis(1));
            }
            Err(_) => return false,
        }
    }
}

/// Forwards what the parent sends into the subgraph node's input port into the child's ingress of
/// its name, until `stop` crossed, the node drains with nothing left to take (then `stop` is
/// sent), the parent's edge closed without `stop` (which ends no input, as on the run loop's
/// edges), the node is cancelled or the child ended.
async fn ingress_bridge<P: From<ControlKind> + Send>(
    mut parent: Box<dyn InputEndpoint<P>>,
    ingress: Box<dyn OutputEndpoint<P>>,
    bridges: Arc<Bridges>,
    crossing: Crossing,
) {
    loop {
        let next = next(
            parent.as_mut(),
            &bridges,
            |state| (state.cancel || state.child_ended).then_some(Next::End),
            |state| state.drain.then_some(Next::Drain),
        )
        .await;
        match next {
            Next::Received(Some(envelope)) => {
                let end = envelope.kind == ControlKind::Stop.as_str();
                if !send(ingress.as_ref(), crossing.cross(envelope), &bridges).await || end {
                    return;
                }
            }
            Next::Drain => {
                let _taken_or_child_gone = send(ingress.as_ref(), crossing.stop(), &bridges).await;
                return;
            }
            Next::Received(None) | Next::End => return,
        }
    }
}

/// Forwards what the child sends out of its egress into the subgraph node's output port of its
/// name, until the child's end of the egress is gone or the node is cancelled.
async fn egress_bridge<P: Send>(
    mut egress: Box<dyn InputEndpoint<P>>,
    parent: Box<dyn OutputEndpoint<P>>,
    bridges: Arc<Bridges>,
    crossing: Crossing,
) {
    loop {
        let next = next(
            egress.as_mut(),
            &bridges,
            |state| state.cancel.then_some(Next::End),
            |_| None,
        )
        .await;
        match next {
            Next::Received(Some(envelope)) => {
                if !send(parent.as_ref(), crossing.cross(envelope), &bridges).await {
                    return;
                }
            }
            Next::Received(None) | Next::Drain | Next::End => return,
        }
    }
}

/// Runs the child's loop and `bridges` together until the child is terminal and every bridge has
/// ended; once the child is terminal, the ingresses end.
async fn run_child<P: Send + 'static>(
    child: &mut RunController<P>,
    mut tasks: Vec<Pin<Box<dyn Future<Output = ()> + Send>>>,
    bridges: &Bridges,
) -> Result<(), RunError> {
    let mut drive = pin!(child.drive());
    let mut result = None;
    future::poll_fn(|cx| {
        if result.is_none()
            && let Poll::Ready(answer) = drive.as_mut().poll(cx)
        {
            result = Some(answer);
            bridges.update(|state| state.child_ended = true);
        }
        tasks.retain_mut(|task| task.as_mut().poll(cx).is_pending());
        if result.is_some() && tasks.is_empty() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
    result.unwrap_or(Ok(()))
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes.
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park();
    }
}
