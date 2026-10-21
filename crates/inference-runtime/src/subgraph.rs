//! The subgraph node (`docs/design/inference.md` 544, A-14 at 1150): a node of a parent run that
//! runs its child pipeline as a nested child run, so that the run boundary is the cancellation and
//! tracing boundary.
//!
//! A [`SubgraphNode`] is the implementation of a node declared with
//! `inference.graph.AddSubgraphNode`, whose ports planning matched one to one with its child
//! pipeline's exposures, its usage port aside: the node's structured output port `usage` also
//! carries every usage egress of the child no port of the node names
//! ([`Declaration::forwarded_usage`]), so the usage of every model call of the child reaches the
//! parent and counts in the parent run's total. It drives a [`RunController`] of the child from
//! its own lifecycle:
//!
//! | step of the subgraph node | what the child run does |
//! |---|---|
//! | `describe`, `described` | nothing yet; the node keeps its declaration and its instance |
//! | `prepare` | created under the node's instance as its parent (`NestRun`), validated, prepared |
//! | `start` | activated; the parent edges arriving at each input port are merged into the child ingress of its name, and the child egress of each output port's name, with every usage egress the usage port forwards merged into it, is fanned out to every parent edge leaving it; every child egress of an output port no parent edge leaves is read and what it sends dropped; the child's run loop runs. A refused start cancels and stops the child |
//! | `drain` | each ingress still open forwards what its parent edges hold, sends `stop` and ends; only then is a drain asked for, so input the parent admitted before its drain is never overtaken by it at any nesting level (`inference.graph.DrainRun`: in-flight events complete); the step returns once the child run is terminal |
//! | `cancel` | cancelled |
//! | `stop` | returns once the child run is terminal and its queues are dropped |
//!
//! A child run that failed fails the subgraph node's drain, or its prepare or start, with a
//! [`NodeError`] carrying the child's reason, which names the child node; the parent run then fails
//! with a reason naming the subgraph node. A child run that did not complete when drained fails
//! the drain too. A refused prepare or start leaves the child run terminal, every instance of it
//! terminal and every queue of it dropped, and records it in the [`SubgraphHandle`], since the
//! parent calls a failed node no further.
//!
//! A bridge's every wait gives way to a cancel, except where it reads a usage egress of the child
//! ([`Declaration::is_usage_egress`]): a model node sends the record of the turn a cancel cut
//! after its own cancel, so the bridge goes on reading such an egress until the child run is
//! terminal and the egress closes, and sends what it reads there without giving way, so that a
//! cancel of the parent loses no usage of the child. A full `fail` queue at a bridge is
//! back-pressure, not loss: the bridge holds the envelope until the queue has room (see its
//! private `send`).
//!
//! An output port of the node that no parent edge leaves, its `usage` port among them, which
//! validation admits when the port is optional, is the node's as a model node's port with no edge
//! is the model node's: what the child sends out of the egresses the port takes reaches no parent
//! edge, and the parent run is not failed for it. A bridge with no parent edge reads those
//! egresses as any egress bridge does, a usage egress through a cancel until it closes, and drops
//! what it reads, so that the child's nodes are never refused on a send there and the child run
//! completes. Nothing in the parent carries a usage record dropped there, as nothing carries the
//! record a model node with no edge on its `usage` port does not send; the child's own nodes sent
//! it, so a [`Recorder`](crate::record::Recorder) wrapping them records it in the child.
//!
//! Tracing needs no library: every envelope inside the child carries the child run's identity, as
//! its nodes stamp it. An envelope crossing the boundary, in either direction, is sent on the other
//! side with that side's run identity, a new event identity, the subgraph node as its producer, the
//! port or exposure name it crosses to as its port, and the crossed envelope's event identity as
//! its `causation_id`; its stream, sequence, generation, kind and payload are kept, so a child's
//! generation epochs stay the child's. A usage record crossing to the parent therefore travels on
//! the parent run, whose total counts it, and still names the child run it was made in in its
//! payload, so the child's total stays derivable. A run's parent is read from its controller
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

use inference_core::endpoint::{InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_pipeline::{
    Declaration, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeInstanceId,
    PlannedPipeline, PortId, USAGE_PORT,
};

use crate::controller::{Ingress, RunController, RunError};
use crate::drive::RunRequests;
use crate::queue::{EpochLookup, Room};
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

    /// The child run as the subgraph node left it: when its stop returned, or when a refused
    /// prepare or start wound it down, after which the node is called no further; none before.
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
    /// The subgraph node is cancelled: every bridge ends, forwarding nothing more, but for what
    /// an egress bridge still reads on a usage egress until it closes.
    cancel: bool,
    /// The child run is terminal: each ingress ends.
    child_ended: bool,
    /// The thread of the child has returned.
    done: bool,
    /// The ingress bridges that have not ended.
    ingresses: usize,
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
        if answer.is_none() && !state.1.iter().any(|known| known.will_wake(cx.waker())) {
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

    /// Returns once every ingress bridge has ended, or the thread of the child has returned.
    async fn ingresses_ended(&self) {
        future::poll_fn(|cx| {
            match self.read(cx, |state| {
                (state.ingresses == 0 || state.done).then_some(())
            }) {
                Some(()) => Poll::Ready(()),
                None => Poll::Pending,
            }
        })
        .await;
    }
}

/// One ingress bridge counted open in its [`Bridges`] until it is dropped, whichever way it ends.
struct OpenIngress(Arc<Bridges>);

impl OpenIngress {
    fn new(bridges: &Arc<Bridges>) -> Self {
        bridges.update(|state| state.ingresses += 1);
        Self(Arc::clone(bridges))
    }
}

impl Drop for OpenIngress {
    fn drop(&mut self) {
        self.0
            .update(|state| state.ingresses = state.ingresses.saturating_sub(1));
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

impl<P: From<ControlKind> + Clone + Send + 'static> SubgraphNode<P> {
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
            self.wound_down(child);
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
        match self.bridges(&mut child, endpoints) {
            Ok(tasks) => {
                if let Err(error) = child.activate().await {
                    let refused = Self::refused(&child, &error);
                    drop(tasks);
                    let _already_failed_or_cancelled = child.cancel().await;
                    self.wound_down(child);
                    return Err(refused);
                }
                let requests = child.requests();
                let bridges = Arc::clone(&tasks.bridges);
                let thread = thread::spawn(move || {
                    let result = block_on(run_child(&mut child, tasks.tasks, &tasks.bridges));
                    tasks.bridges.update(|state| state.done = true);
                    (child, result)
                });
                self.state = Child::Running {
                    requests,
                    bridges,
                    thread,
                };
                Ok(())
            }
            Err(refused) => {
                let _cancelled = child.cancel().await;
                self.wound_down(child);
                Err(refused)
            }
        }
    }

    /// The bridges between the node's edge endpoints and the child's exposures: one per exposure
    /// the node's ports name, an ingress merging every parent edge into its input port of that
    /// name, an egress fanned out to every parent edge leaving its output port of that name; and
    /// one with no parent edge reading every egress of the child no other bridge took, those of
    /// the output ports no parent edge leaves, and dropping what it reads.
    ///
    /// # Errors
    ///
    /// The error naming a port whose name no exposure of the child carries, or the node when it
    /// was not described; the child is then to be wound down.
    fn bridges(
        &self,
        child: &mut RunController<P>,
        endpoints: NodeEndpoints<P>,
    ) -> Result<Tasks, NodeError> {
        let node_id = self.node.as_ref().map(Node::node_id).ok_or_else(|| {
            NodeError::new("the subgraph node was started before it was described")
        })?;
        let name = |port_id: PortId| {
            self.declaration
                .port(port_id)
                .map(|port| port.name().to_owned())
                .unwrap_or_default()
        };
        let mut inputs: Grouped<Box<dyn InputEndpoint<P>>> = Vec::new();
        for input in endpoints.inputs {
            group(&mut inputs, name(input.port_id), input.endpoint);
        }
        let mut outputs: Grouped<Box<dyn OutputEndpoint<P>>> = Vec::new();
        for output in endpoints.outputs {
            group(&mut outputs, name(output.port_id), output.endpoint);
        }
        let mut ends = child.exposed();
        let child_run = child.run().run_id();
        let bridges = Arc::new(Bridges::default());
        let mut tasks: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = Vec::new();
        for (exposure, parents) in inputs {
            let Some(Ingress {
                endpoint: ingress,
                room,
                ..
            }) = ends.take_ingress(&exposure)
            else {
                return Err(NodeError::new(format!(
                    "the child has no ingress named `{exposure}` for the node's input port of \
                     that name"
                )));
            };
            tasks.push(Box::pin(ingress_bridge(
                parents,
                ingress,
                room,
                OpenIngress::new(&bridges),
                Crossing {
                    run_id: child_run,
                    producer: node_id,
                    port: exposure,
                },
            )));
        }
        let forwarded: Vec<String> = self
            .declaration
            .forwarded_usage(node_id)
            .iter()
            .map(|exposure| exposure.name().to_owned())
            .collect();
        let child_id = self.child.pipeline_id();
        let carries_usage = |name: &str| {
            self.declaration.exposures().any(|exposure| {
                exposure.pipeline_id() == child_id
                    && exposure.name() == name
                    && self.declaration.is_usage_egress(exposure)
            })
        };
        for (port, parents) in outputs {
            let mut names = vec![port.clone()];
            if port == USAGE_PORT {
                names.extend(forwarded.iter().cloned());
            }
            let egresses: Vec<ChildEgress<P>> = names
                .iter()
                .filter_map(|name| {
                    ends.egress(name).map(|endpoint| ChildEgress {
                        endpoint,
                        usage: carries_usage(name),
                    })
                })
                .collect();
            if egresses.is_empty() {
                return Err(NodeError::new(format!(
                    "the child has no egress named `{port}` for the node's output port of that \
                     name"
                )));
            }
            tasks.push(Box::pin(egress_bridge(
                egresses,
                parents,
                Arc::clone(&bridges),
                Crossing {
                    run_id: self.run_id,
                    producer: node_id,
                    port,
                },
            )));
        }
        let unconnected: Vec<ChildEgress<P>> = std::mem::take(&mut ends.egresses)
            .into_iter()
            .map(|egress| ChildEgress {
                usage: carries_usage(&egress.name),
                endpoint: egress.endpoint,
            })
            .collect();
        if !unconnected.is_empty() {
            tasks.push(Box::pin(egress_bridge(
                unconnected,
                Vec::new(),
                Arc::clone(&bridges),
                Crossing {
                    run_id: self.run_id,
                    producer: node_id,
                    port: String::new(),
                },
            )));
        }
        Ok(Tasks { bridges, tasks })
    }

    /// Keeps `child`, terminal or refused, and records it as the node leaves it: a step that
    /// refused fails the node's instance, which the parent calls no further, so this is the last
    /// the node does with its child.
    fn wound_down(&mut self, child: RunController<P>) {
        self.record(&child);
        self.state = Child::Ended(child);
    }

    /// Records what `child` has become in the node's handle.
    fn record(&self, child: &RunController<P>) {
        let at_stop = ChildAtStop {
            run_id: child.run().run_id(),
            parent: child.parent(),
            state: child.run().state(),
            instances: child
                .instances()
                .map(crate::run::NodeInstance::state)
                .collect(),
            queues_dropped: child.queues().iter().all(|queue| queue.is_dropped())
                && child
                    .exposure_queues()
                    .iter()
                    .all(|queue| queue.is_dropped()),
        };
        self.handle.seen().at_stop = Some(at_stop);
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
            bridges.ingresses_ended().await;
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
        if let Child::Prepared(child) | Child::Ended(child) = &self.state {
            self.record(child);
        }
    }
}

/// The bridges of a child about to run: what they are told, and their tasks.
struct Tasks {
    bridges: Arc<Bridges>,
    tasks: Vec<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

/// Endpoints grouped by the name of the port they belong to, in the order the names first appear.
type Grouped<E> = Vec<(String, Vec<E>)>;

/// Adds `endpoint` to the group of `name` in `groups`, in the order the names first appear.
fn group<E>(groups: &mut Grouped<E>, name: String, endpoint: E) {
    match groups.iter_mut().find(|(known, _)| *known == name) {
        Some((_, endpoints)) => endpoints.push(endpoint),
        None => groups.push((name, vec![endpoint])),
    }
}

impl<P: From<ControlKind> + Clone + Send + 'static> NodeImplementation<P> for SubgraphNode<P> {
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

/// How a bridge's send ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sent {
    /// The queue took the envelope, or discarded it under its overload policy.
    Taken,
    /// The queue refused it otherwise: closed, or the sequence did not increase.
    Refused,
    /// The subgraph node was cancelled before the queue took it; it is dropped.
    Cancelled,
}

/// Sends `envelope` on `output` as a bridge does, racing every wait against the node's cancel when
/// it `yields`, so that a cancel ends a bridge whose queue holds a send waiting for room (`pause`).
/// A usage record's send does not yield: it waits for the queue to take it or refuse it, closed,
/// so that a cancel loses no usage (see [`egress_bridge`]).
///
/// A `fail` queue that is full answers [`SendError::Full`]. At a bridge that answer is
/// back-pressure, not loss: the envelope was already taken from the other side's queue, whose
/// producer saw its send succeed, so failing here would lose it with nobody told, and failing the
/// subgraph node would fail the parent for the size of a queue the parent does not see. The bridge
/// therefore holds the envelope and sends it again once the queue has room: woken by the
/// receiver's next take where the bridge holds the queue's `room` (a child ingress), otherwise by
/// yielding once to the other tasks of its thread (a parent edge, which hands the node only its
/// sending end). Either way it gives way to the child's run loop and the other bridges instead of
/// blocking their thread, and the other side's queue stays full meanwhile, which holds its
/// producer under that queue's own policy.
async fn send<P>(
    output: &dyn OutputEndpoint<P>,
    envelope: EventEnvelope<P>,
    room: Option<&Room<P>>,
    bridges: &Bridges,
    yields: bool,
) -> Sent {
    let cancelled = |cx: &Context<'_>| {
        yields
            && bridges
                .read(cx, |state| state.cancel.then_some(()))
                .is_some()
    };
    let mut envelope = envelope;
    loop {
        let answer = {
            let mut sending = output.send(envelope);
            future::poll_fn(|cx| {
                if cancelled(cx) {
                    return Poll::Ready(None);
                }
                sending.as_mut().poll(cx).map(Some)
            })
            .await
        };
        match answer {
            None => return Sent::Cancelled,
            Some(Ok(())) => return Sent::Taken,
            Some(Err(SendError::Full(refused))) => {
                envelope = refused;
                let mut yielded = false;
                let gave_way = future::poll_fn(|cx| {
                    if cancelled(cx) {
                        return Poll::Ready(false);
                    }
                    match room {
                        Some(room) => room.poll_room(cx).map(|()| true),
                        None if yielded => Poll::Ready(true),
                        None => {
                            yielded = true;
                            cx.waker().wake_by_ref();
                            Poll::Pending
                        }
                    }
                })
                .await;
                if !gave_way {
                    return Sent::Cancelled;
                }
            }
            Some(Err(_)) => return Sent::Refused,
        }
    }
}

/// One parent edge's next envelope, with the edge's receiving end handed back.
type Receiving<P> =
    Pin<Box<dyn Future<Output = (Box<dyn InputEndpoint<P>>, Option<EventEnvelope<P>>)> + Send>>;

fn receiving<P: Send + 'static>(mut input: Box<dyn InputEndpoint<P>>) -> Receiving<P> {
    Box::pin(async move {
        let received = input.recv().await;
        (input, received)
    })
}

/// What the parent edges of an ingress have next.
enum Merged<P> {
    Received(Box<dyn InputEndpoint<P>>, Option<Box<EventEnvelope<P>>>),
    Drain,
    End,
}

/// Forwards what the parent sends into the subgraph node's input port, along every parent edge
/// arriving at it, into the child's ingress of its name. An edge ends at its `stop` or when it
/// closes; once every edge ended, `stop` crosses if every one of them ended with `stop`, and none
/// does if one closed without it (which ends no input, as on the run loop's own edges). It also
/// ends when the node drains with nothing left to take on any edge (then `stop` is sent), is
/// cancelled, or its child ended. The node's drain asks the child run for its drain only once
/// every ingress bridge has ended, which `open` counts: a child that is itself a subgraph node's
/// parent would otherwise drain that node, whose ingress bridge then finds nothing on its edge and
/// sends `stop`, before this bridge has forwarded what it took.
async fn ingress_bridge<P: From<ControlKind> + Send + 'static>(
    parents: Vec<Box<dyn InputEndpoint<P>>>,
    ingress: Box<dyn OutputEndpoint<P>>,
    room: Option<Room<P>>,
    open: OpenIngress,
    crossing: Crossing,
) {
    let bridges = &open.0;
    let mut pending: Vec<Receiving<P>> = parents.into_iter().map(receiving).collect();
    let mut closed = false;
    loop {
        if pending.is_empty() {
            if !closed {
                let _taken_or_child_gone = send(
                    ingress.as_ref(),
                    crossing.stop(),
                    room.as_ref(),
                    bridges,
                    true,
                )
                .await;
            }
            return;
        }
        let next = future::poll_fn(|cx| {
            if bridges
                .read(cx, |state| {
                    (state.cancel || state.child_ended).then_some(())
                })
                .is_some()
            {
                return Poll::Ready(Merged::End);
            }
            for index in 0..pending.len() {
                if let Poll::Ready((input, received)) = pending[index].as_mut().poll(cx) {
                    drop(pending.swap_remove(index));
                    return Poll::Ready(Merged::Received(input, received.map(Box::new)));
                }
            }
            bridges
                .read(cx, |state| state.drain.then_some(()))
                .map_or(Poll::Pending, |()| Poll::Ready(Merged::Drain))
        })
        .await;
        match next {
            Merged::Received(input, Some(envelope)) => {
                if envelope.kind == ControlKind::Stop.as_str() {
                    continue;
                }
                match send(
                    ingress.as_ref(),
                    crossing.cross(*envelope),
                    room.as_ref(),
                    bridges,
                    true,
                )
                .await
                {
                    Sent::Taken => pending.push(receiving(input)),
                    Sent::Refused | Sent::Cancelled => return,
                }
            }
            Merged::Received(_, None) => closed = true,
            Merged::Drain => {
                let _taken_or_child_gone = send(
                    ingress.as_ref(),
                    crossing.stop(),
                    room.as_ref(),
                    bridges,
                    true,
                )
                .await;
                return;
            }
            Merged::End => return,
        }
    }
}

/// A child egress an egress bridge reads, and whether it is a usage egress
/// ([`Declaration::is_usage_egress`]).
struct ChildEgress<P> {
    endpoint: Box<dyn InputEndpoint<P>>,
    usage: bool,
}

/// One child egress's next envelope, with the egress's index among those a bridge merges and its
/// receiving end handed back.
type Indexed<P> = Pin<
    Box<dyn Future<Output = (usize, Box<dyn InputEndpoint<P>>, Option<EventEnvelope<P>>)> + Send>,
>;

fn indexed<P: Send + 'static>(index: usize, mut egress: Box<dyn InputEndpoint<P>>) -> Indexed<P> {
    Box::pin(async move {
        let received = egress.recv().await;
        (index, egress, received)
    })
}

/// What a bridge's merged egresses have next.
enum Next<P> {
    Received(
        usize,
        Box<dyn InputEndpoint<P>>,
        Option<Box<EventEnvelope<P>>>,
    ),
    Cancelled,
}

/// Forwards what the child sends out of `egresses` into every parent edge leaving the subgraph
/// node's output port, each copy a crossing of its own, until the child's end of every egress is
/// gone or every parent edge refused. An edge that refuses is sent nothing more; the others go on.
/// Given no parent edge, the bridge reads until the child's end of every egress is gone and
/// drops what it reads, a cancel ending it as below.
///
/// A cancel of the node ends the bridge at once, forwarding nothing more, unless it reads a usage
/// egress. Usage is never lost to a cancel: a model node sends the record of the turn the cancel
/// cut after its own cancel (`story:llm-usage-on-cancel`), so at the cancel the bridge stops
/// reading every other egress and goes on reading its usage egresses until the child run is
/// terminal and they close, and its sends of their envelopes do not give way to the cancel: each
/// waits until the parent edge takes it or refuses it, closed.
///
/// `egresses` is the child's egress of the port's name, or, for the node's usage port, that and
/// every usage egress the port forwards ([`Declaration::forwarded_usage`]), merged as they arrive,
/// so every usage record of the child crosses once. A `stop` ends one egress's input: it crosses
/// once every egress has sent one, the last to arrive, and none crosses if an egress ended
/// without it, as on the ingress side; so the parent sees one end of input whatever it merges.
async fn egress_bridge<P: Clone + Send + 'static>(
    egresses: Vec<ChildEgress<P>>,
    parents: Vec<Box<dyn OutputEndpoint<P>>>,
    bridges: Arc<Bridges>,
    crossing: Crossing,
) {
    let mut parents = parents;
    let discards = parents.is_empty();
    let mut stopped = vec![false; egresses.len()];
    let usage: Vec<bool> = egresses.iter().map(|egress| egress.usage).collect();
    let mut cancelled = false;
    let mut pending: Vec<(usize, Indexed<P>)> = egresses
        .into_iter()
        .enumerate()
        .map(|(index, egress)| (index, indexed(index, egress.endpoint)))
        .collect();
    while !pending.is_empty() {
        let next = future::poll_fn(|cx| {
            if !cancelled
                && bridges
                    .read(cx, |state| state.cancel.then_some(()))
                    .is_some()
            {
                return Poll::Ready(Next::Cancelled);
            }
            for at in 0..pending.len() {
                if let Poll::Ready((index, egress, received)) = pending[at].1.as_mut().poll(cx) {
                    drop(pending.swap_remove(at));
                    return Poll::Ready(Next::Received(index, egress, received.map(Box::new)));
                }
            }
            Poll::Pending
        })
        .await;
        let (index, egress, received) = match next {
            Next::Cancelled => {
                cancelled = true;
                pending.retain(|(index, _)| usage[*index]);
                continue;
            }
            Next::Received(index, egress, received) => (index, egress, received),
        };
        let Some(envelope) = received else {
            continue;
        };
        pending.push((index, indexed(index, egress)));
        if envelope.kind == ControlKind::Stop.as_str() {
            stopped[index] = true;
            if !stopped.iter().all(|stopped| *stopped) {
                continue;
            }
        }
        let mut kept = Vec::with_capacity(parents.len());
        for parent in parents {
            match send(
                parent.as_ref(),
                crossing.cross((*envelope).clone()),
                None,
                &bridges,
                !usage[index],
            )
            .await
            {
                Sent::Taken => kept.push(parent),
                Sent::Refused => {}
                Sent::Cancelled => return,
            }
        }
        if kept.is_empty() && !discards {
            return;
        }
        parents = kept;
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
