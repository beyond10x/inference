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
//! | `start` | activated; the parent edges arriving at each input port are merged into the child ingress of its name, and the child egress of each output port's name, with every usage egress the usage port forwards and the sink of every unconnected child `usage` port merged into it, is fanned out to every parent edge leaving it; every child egress of an output port other than `usage` that no parent edge leaves is read and what it sends dropped; the child's run loop runs. A refused start cancels and stops the child |
//! | `drain` | each ingress still open forwards what its parent edges hold, sends `stop` and ends; only then is a drain asked for, so input the parent admitted before its drain is never overtaken by it at any nesting level (`inference.graph.DrainRun`: in-flight events complete); the step returns once the child run is terminal |
//! | `cancel` | cancelled |
//! | `stop` | returns once the child run is terminal and its queues are dropped |
//!
//! A child run that failed fails the subgraph node's drain, or its prepare or start, with a
//! [`NodeError`] carrying the child's reason, which names the child node; the parent run then fails
//! with a reason naming the subgraph node. A child run that did not complete when drained fails
//! the drain too, and so does an output port every parent edge of which refused what the child
//! sent there outside a cancel, as a model node fails on a send its edge refuses, so that the
//! parent run ends the same whether or not the child's node had sent all it had before the
//! refusal. A refused prepare or start leaves the child run terminal, every instance of it
//! terminal and every queue of it dropped, and records it in the [`SubgraphHandle`], since the
//! parent calls a failed node no further.
//!
//! A bridge's every wait gives way to a cancel, except where it reads a usage egress of the child
//! ([`Declaration::is_usage_egress`]): a model node sends the record of the turn a cancel cut
//! after its own cancel, so the bridge goes on reading such an egress until the child run is
//! terminal and the egress closes, and sends what it reads there without giving way to the
//! cancel, so that a cancel of the parent loses no usage of the child. Such a send gives way only
//! once the node is stopped and [`USAGE_STOP_GRACE`] has passed with the parent edge still full,
//! as when its consumer holds its receiver unread, so that the node's stop, and the parent's
//! wind-down with it, cannot hang on it; each record given up so is counted
//! ([`SubgraphHandle::dropped_usage`]). A full `fail` queue at a bridge is back-pressure, not
//! loss: the bridge holds the envelope until the queue has room, waiting on a timer of the child's
//! thread between its tries where it cannot be woken by the room (see its private `send`).
//!
//! A subgraph node dropped while its child runs, as when its parent's controller is dropped
//! without a cancel, cancels the child and waits up to [`CHILD_DROP_BOUND`] for the child's
//! thread to return, so the child's nodes do not outlive their parent.
//!
//! An output port of the node other than `usage` that no parent edge leaves, which validation
//! admits when the port is optional, is the node's as a model node's port with no edge is the
//! model node's: what the child sends out of the egresses the port takes reaches no parent edge,
//! and the parent run is not failed for it. A bridge with no parent edge reads those egresses as
//! any egress bridge does, a usage egress through a cancel until it closes, and drops what it
//! reads, so that the child's nodes are never refused on a send there and the child run
//! completes. Every egress bridge, with parent edges or none, ends once the child run has
//! returned and its egresses hold nothing more, even where a node of the child kept its egress
//! end past its stop.
//!
//! Usage is not dropped that way (`story:usage-unconnected-port-counted`). The node's `usage`
//! port with no parent edge is handed a sink by the parent run ([`crate::usage_sink`]), as a model
//! node's is, so the usage it forwards reaches the parent's caller, which counts it in the parent
//! run's total. A `usage` port of the child with no edge and no exposure is handed a sink by the
//! child run, and the node's `usage` port forwards what that sink holds with the child's usage
//! egresses. Only a node with no `usage` port, whose child exposes no usage egress, has nowhere to
//! send what such a sink holds and drops it; [`crate::usage_sink::uncounted_usage`] names it, so
//! that the caller can mark its total partial.
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{
    ClosedToSenders, InputEndpoint, KnownContents, OutputEndpoint, SendError,
};
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

/// How long dropping a [`SubgraphNode`] whose child runs waits for the child's thread to end.
pub const CHILD_DROP_BOUND: Duration = Duration::from_secs(5);

/// How long, once the subgraph node is stopped, a usage send of a bridge waits for room on a
/// parent edge before it gives the record up.
pub const USAGE_STOP_GRACE: Duration = Duration::from_secs(1);

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
    dropped_usage: u64,
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

    /// How many usage records of the child the node gave up unsent because the parent edge they
    /// waited on had no room [`USAGE_STOP_GRACE`] after the node's stop, or at its drop; counted
    /// once the child's thread has returned. The caller marks the parent's usage total partial for
    /// them, as for [`crate::usage_sink::uncounted_usage`].
    #[must_use]
    pub fn dropped_usage(&self) -> u64 {
        self.seen().dropped_usage
    }
}

/// What the bridges of a running child are told, shared with them, and the timers of the child's
/// thread.
#[derive(Default)]
struct Bridges {
    state: Mutex<(BridgeState, Vec<Waker>)>,
    /// Notified on every change of the state, for a drop waiting for the thread to return.
    changed: Condvar,
    /// The tasks of the child's thread waiting for an instant, each woken once it passed.
    due: Mutex<Vec<(Instant, Waker)>>,
}

#[derive(Default)]
struct BridgeState {
    /// The subgraph node drains: each ingress still open sends `stop` and ends.
    drain: bool,
    /// The subgraph node is cancelled: every bridge ends, forwarding nothing more, but for what
    /// an egress bridge still reads on a usage egress until it closes.
    cancel: bool,
    /// The child run is terminal: each ingress ends, and each egress bridge once its egresses
    /// hold nothing more.
    child_ended: bool,
    /// The thread of the child has returned.
    done: bool,
    /// The first output port of the node every parent edge of which refused what its egress
    /// bridge sent, outside a cancel: the node's drain fails for it.
    refused: Option<String>,
    /// The ingress bridges that have not ended.
    ingresses: usize,
    /// The subgraph node is stopped or dropped: a send waiting for room gives way, a usage send
    /// once this instant has passed and any other at once.
    stop_deadline: Option<Instant>,
    /// The usage records an egress bridge gave up after the stop deadline, unsent.
    dropped_usage: u64,
}

impl Bridges {
    fn update(&self, change: impl FnOnce(&mut BridgeState)) {
        let wakers = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            change(&mut state.0);
            std::mem::take(&mut state.1)
        };
        self.changed.notify_all();
        wakers.into_iter().for_each(Waker::wake);
    }

    /// Waits up to `bound` for the thread of the child to return: true when it did.
    fn wait_done(&self, bound: Duration) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, _) = self
            .changed
            .wait_timeout_while(state, bound, |state| !state.0.done)
            .unwrap_or_else(PoisonError::into_inner);
        state.0.done
    }

    fn dropped_usage(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .dropped_usage
    }

    /// Wakes the task of `cx` once `at` has passed; the child's thread parks no longer than that.
    fn wake_at(&self, cx: &Context<'_>, at: Instant) {
        let mut due = self.due.lock().unwrap_or_else(PoisonError::into_inner);
        if !due
            .iter()
            .any(|(known, waker)| *known == at && waker.will_wake(cx.waker()))
        {
            due.push((at, cx.waker().clone()));
        }
    }

    /// Wakes every task whose instant has passed and answers the earliest instant still ahead.
    fn fire_due(&self) -> Option<Instant> {
        let now = Instant::now();
        let fired: Vec<Waker> = {
            let mut due = self.due.lock().unwrap_or_else(PoisonError::into_inner);
            let (fired, ahead) = std::mem::take(&mut *due)
                .into_iter()
                .partition::<Vec<_>, _>(|(at, _)| *at <= now);
            *due = ahead;
            fired.into_iter().map(|(_, waker)| waker).collect()
        };
        fired.into_iter().for_each(Waker::wake);
        self.due
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(at, _)| *at)
            .min()
    }

    /// Whether a send waiting for room gives way now: a send that `yields` at the node's cancel
    /// or stop, a usage send once the stop deadline has passed. A usage send before its deadline
    /// has the task of `cx` woken at the deadline; any other answer, at the next change.
    fn gives_way(&self, cx: &Context<'_>, yields: bool) -> bool {
        let deadline = self.read(cx, |state| {
            if yields && (state.cancel || state.stop_deadline.is_some()) {
                Some(None)
            } else {
                state.stop_deadline.map(Some)
            }
        });
        match deadline {
            None => false,
            Some(None) => true,
            Some(Some(at)) if Instant::now() >= at => true,
            Some(Some(at)) => {
                self.wake_at(cx, at);
                false
            }
        }
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

    /// The output port every parent edge of which refused, outside a cancel; see
    /// [`BridgeState::refused`].
    fn refused(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .refused
            .clone()
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
                    let result = block_on(
                        run_child(&mut child, tasks.tasks, &tasks.bridges),
                        &tasks.bridges,
                    );
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
    /// name, an egress fanned out to every parent edge leaving its output port of that name, the
    /// `usage` port's merging the sinks of the child's unconnected `usage` ports too; and one with
    /// no parent edge reading every egress of the child no other bridge took, and those sinks when
    /// the node has no `usage` port, and dropping what it reads.
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
            let mut egresses: Vec<ChildEgress<P>> = names
                .iter()
                .filter_map(|name| {
                    ends.egress(name).map(|endpoint| ChildEgress {
                        endpoint,
                        usage: carries_usage(name),
                        sink: false,
                    })
                })
                .collect();
            if egresses.is_empty() {
                return Err(NodeError::new(format!(
                    "the child has no egress named `{port}` for the node's output port of that \
                     name"
                )));
            }
            if port == USAGE_PORT {
                egresses.extend(std::mem::take(&mut ends.usage).into_iter().map(|sink| {
                    ChildEgress {
                        endpoint: sink.endpoint,
                        usage: true,
                        sink: true,
                    }
                }));
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
                sink: false,
            })
            .chain(ends.usage.into_iter().map(|sink| ChildEgress {
                endpoint: sink.endpoint,
                usage: true,
                sink: true,
            }))
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
        self.handle.seen().dropped_usage += bridges.dropped_usage();
        if let Child::Running { thread, .. } = std::mem::replace(&mut self.state, Child::None) {
            self.state = match thread.join() {
                Ok((child, _answered)) => Child::Ended(child),
                Err(_) => Child::None,
            };
        }
    }

    async fn drain_child(&mut self) -> Result<(), NodeError> {
        let mut refused = None;
        if let Child::Running {
            requests, bridges, ..
        } = &self.state
        {
            bridges.update(|state| state.drain = true);
            bridges.ingresses_ended().await;
            requests.drain();
            refused = Some(Arc::clone(bridges));
        }
        self.ended().await;
        let refused = refused.and_then(|bridges| bridges.refused());
        match &self.state {
            Child::Ended(child) if child.run().state() == RunState::Completed => {
                refused.map_or(Ok(()), |port| {
                    Err(NodeError::new(format!(
                        "every edge leaving the subgraph node's output port `{port}` refused \
                         what the child sent there"
                    )))
                })
            }
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

    /// Stops the child: a bridge send still waiting for room gives way, a usage send once
    /// [`USAGE_STOP_GRACE`] has passed, so that a parent edge whose consumer holds its receiver
    /// unread cannot hold the stop; then waits for the child's thread to return.
    async fn stop_child(&mut self) {
        if let Child::Running { bridges, .. } = &self.state {
            let deadline = Instant::now() + USAGE_STOP_GRACE;
            bridges.update(|state| {
                state.stop_deadline.get_or_insert(deadline);
            });
        }
        self.ended().await;
        if let Child::Prepared(child) = &mut self.state {
            let _cancelled_or_already_terminal = child.cancel().await;
        }
        if let Child::Prepared(child) | Child::Ended(child) = &self.state {
            self.record(child);
        }
    }
}

/// A subgraph node dropped while its child runs, as when its parent's controller is dropped
/// without a cancel, cancels the child run, has every bridge send give way at once, usage sends
/// included, and waits up to [`CHILD_DROP_BOUND`] for the child's thread to return, joining it;
/// a thread still running then, held by a node of the child that does not end on its cancel, is
/// left to end on its own. Usage records given up are counted in the [`SubgraphHandle`].
impl<P> Drop for SubgraphNode<P> {
    fn drop(&mut self) {
        let Child::Running {
            requests,
            bridges,
            thread,
        } = std::mem::replace(&mut self.state, Child::None)
        else {
            return;
        };
        let now = Instant::now();
        bridges.update(|state| {
            state.cancel = true;
            state.stop_deadline = Some(now);
        });
        requests.cancel();
        if bridges.wait_done(CHILD_DROP_BOUND) {
            let _child_and_its_answer = thread.join();
        }
        self.handle.seen().dropped_usage += bridges.dropped_usage();
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
    /// The subgraph node was cancelled, or stopped, before the queue took it; it is dropped.
    Cancelled,
}

/// The first wait before a bridge sends again into a full `fail` queue whose `room` it does not
/// hold, and the longest: each wait doubles the last, up to the longest.
const FULL_RETRY_FIRST: Duration = Duration::from_millis(1);
const FULL_RETRY_LONGEST: Duration = Duration::from_millis(16);

/// Sends `envelope` on `output` as a bridge does, racing every wait against the node's cancel and
/// stop when it `yields`, so that a cancel ends a bridge whose queue holds a send waiting for room
/// (`pause`). A usage record's send does not yield to a cancel: it waits for the queue to take it
/// or refuse it, closed, so that a cancel loses no usage (see [`egress_bridge`]). It gives way
/// only once the node is stopped and [`USAGE_STOP_GRACE`] has passed, or at once when the node is
/// dropped, so that a parent consumer holding its receiver unread cannot hold the node's stop, and
/// with it the parent's wind-down, forever.
///
/// A `fail` queue that is full answers [`SendError::Full`]. At a bridge that answer is
/// back-pressure, not loss: the envelope was already taken from the other side's queue, whose
/// producer saw its send succeed, so failing here would lose it with nobody told, and failing the
/// subgraph node would fail the parent for the size of a queue the parent does not see. The bridge
/// therefore holds the envelope and sends it again once the queue has room: woken by the
/// receiver's next take where the bridge holds the queue's `room` (a child ingress), otherwise
/// after a wait on a timer of the child's thread, [`FULL_RETRY_FIRST`] doubling up to
/// [`FULL_RETRY_LONGEST`] (a parent edge, which hands the node only its sending end), during which
/// the thread parks, so that a consumer slower than the child does not spin it. Either way it
/// gives way to the child's run loop and the other bridges instead of blocking their thread, and
/// the other side's queue stays full meanwhile, which holds its producer under that queue's own
/// policy.
async fn send<P>(
    output: &dyn OutputEndpoint<P>,
    envelope: EventEnvelope<P>,
    room: Option<&Room<P>>,
    bridges: &Bridges,
    yields: bool,
) -> Sent {
    let cancelled = |cx: &Context<'_>| bridges.gives_way(cx, yields);
    let mut envelope = envelope;
    let mut wait = FULL_RETRY_FIRST;
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
                let again = Instant::now() + wait;
                wait = (wait * 2).min(FULL_RETRY_LONGEST);
                let gave_way = future::poll_fn(|cx| {
                    if cancelled(cx) {
                        return Poll::Ready(false);
                    }
                    match room {
                        Some(room) => room.poll_room(cx).map(|()| true),
                        None if Instant::now() >= again => Poll::Ready(true),
                        None => {
                            bridges.wake_at(cx, again);
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
    /// A receive of a parent edge completed: the endpoint, what it answered, and whether the
    /// bridge closed that edge to its senders at the drain, whose `None` ends the edge for the
    /// drain rather than closing it without `stop`.
    Received(
        Box<dyn InputEndpoint<P>>,
        Option<Box<EventEnvelope<P>>>,
        bool,
    ),
    Drain,
    End,
}

/// A parent edge of an ingress bridge, as the bridge holds it between polls.
enum ParentEdge<P> {
    /// An endpoint that answered [`KnownContents::Unknown`] when the bridge last held it: one
    /// receive of it in flight, kept across polls until it completes, as `recv` is awaited.
    Receiving(Receiving<P>),
    /// An endpoint that knows whether its edge holds anything: held by the bridge, which makes a
    /// receive of it on each poll and drops it while pending, so that it can ask the endpoint
    /// between receives ([`InputEndpoint::known_contents`]).
    Known(Box<dyn InputEndpoint<P>>),
    /// A known edge the bridge closed to its senders at the drain, which answered
    /// [`ClosedToSenders::Holding`]: received until its receive answers `None`.
    ClosedToSenders(Box<dyn InputEndpoint<P>>),
}

/// Closes every edge held as known to its senders ([`InputEndpoint::close_to_senders`]): no
/// `recv` future of such an edge is alive between polls. An edge that answers
/// [`ClosedToSenders::Empty`] ends for the drain and is dropped; one that answers
/// [`ClosedToSenders::Holding`] is received until `None`; one that answers
/// [`ClosedToSenders::Unsupported`], and an edge whose receive stays in flight, keeps the path it
/// was on. Answers whether an edge ended for the drain.
fn close_known_to_senders<P>(edges: &mut Vec<ParentEdge<P>>) -> bool {
    let mut ended = false;
    for edge in std::mem::take(edges) {
        match edge {
            ParentEdge::Known(mut input) => match input.close_to_senders() {
                ClosedToSenders::Empty => ended = true,
                ClosedToSenders::Holding => edges.push(ParentEdge::ClosedToSenders(input)),
                ClosedToSenders::Unsupported => edges.push(ParentEdge::Known(input)),
            },
            edge @ (ParentEdge::Receiving(_) | ParentEdge::ClosedToSenders(_)) => edges.push(edge),
        }
    }
    ended
}

/// `input` as the bridge holds it, by what it answers now.
fn parent_edge<P: Send + 'static>(input: Box<dyn InputEndpoint<P>>) -> ParentEdge<P> {
    match input.known_contents() {
        KnownContents::Unknown => ParentEdge::Receiving(receiving(input)),
        KnownContents::Empty | KnownContents::NotEmpty => ParentEdge::Known(input),
    }
}

/// One parent edge polled once.
enum Polled<P> {
    /// Its receive completed: the endpoint, what the receive answered, and whether the bridge
    /// closed the edge to its senders.
    Ready(Box<dyn InputEndpoint<P>>, Option<EventEnvelope<P>>, bool),
    /// Its receive is pending: the edge, and what its endpoint says the edge holds, unknown for
    /// an edge whose receive stays in flight, and not empty for an edge closed to its senders
    /// that still holds what it said.
    Pending(ParentEdge<P>, KnownContents),
}

impl<P> Polled<P> {
    /// A completed receive as the bridge merges it.
    fn merged(
        input: Box<dyn InputEndpoint<P>>,
        received: Option<EventEnvelope<P>>,
        closed: bool,
    ) -> Merged<P> {
        Merged::Received(input, received.map(Box::new), closed)
    }
}

fn poll_edge<P>(edge: ParentEdge<P>, cx: &mut Context<'_>) -> Polled<P> {
    match edge {
        ParentEdge::Receiving(mut receiving) => match receiving.as_mut().poll(cx) {
            Poll::Ready((input, received)) => Polled::Ready(input, received, false),
            Poll::Pending => {
                Polled::Pending(ParentEdge::Receiving(receiving), KnownContents::Unknown)
            }
        },
        ParentEdge::Known(mut input) => {
            let polled = input.recv().as_mut().poll(cx);
            match polled {
                Poll::Ready(received) => Polled::Ready(input, received, false),
                Poll::Pending => {
                    let known = input.known_contents();
                    Polled::Pending(ParentEdge::Known(input), known)
                }
            }
        }
        ParentEdge::ClosedToSenders(mut input) => {
            let polled = input.recv().as_mut().poll(cx);
            match polled {
                Poll::Ready(received) => Polled::Ready(input, received, true),
                Poll::Pending => {
                    Polled::Pending(ParentEdge::ClosedToSenders(input), KnownContents::NotEmpty)
                }
            }
        }
    }
}

/// Polls each edge once with `cx`, in order, and answers the first whose receive completed.
fn next_received<P>(edges: &mut Vec<ParentEdge<P>>, cx: &mut Context<'_>) -> Option<Merged<P>> {
    let mut merged = None;
    for edge in std::mem::take(edges) {
        if merged.is_some() {
            edges.push(edge);
            continue;
        }
        match poll_edge(edge, cx) {
            Polled::Ready(input, received, closed) => {
                merged = Some(Polled::merged(input, received, closed));
            }
            Polled::Pending(edge, _) => edges.push(edge),
        }
    }
    merged
}

/// What one poll of every edge came to while the node drains, the first that applies.
enum Round<P> {
    /// A receive completed.
    Received(Merged<P>),
    /// An endpoint said its edge holds an envelope: the bridge waits for its receive.
    Holding,
    /// A receive of an endpoint that does not know answered `Pending` and woke the bridge's task
    /// in the same poll.
    Woke,
    /// Every edge is taken as empty.
    Empty,
}

/// Polls each edge once while the node drains, each with a [`Woken`] of its own, so that an
/// edge whose endpoint knows it is empty is not polled again because its receive woke itself.
fn drain_round<P>(edges: &mut Vec<ParentEdge<P>>, cx: &Context<'_>) -> Round<P> {
    let mut round = Round::Empty;
    for edge in std::mem::take(edges) {
        if matches!(round, Round::Received(_)) {
            edges.push(edge);
            continue;
        }
        let woken = Arc::new(Woken {
            woken: AtomicBool::new(false),
            task: cx.waker().clone(),
        });
        let waker = Waker::from(Arc::clone(&woken));
        match poll_edge(edge, &mut Context::from_waker(&waker)) {
            Polled::Ready(input, received, closed) => {
                round = Round::Received(Polled::merged(input, received, closed));
            }
            Polled::Pending(edge, known) => {
                edges.push(edge);
                match known {
                    KnownContents::NotEmpty => round = Round::Holding,
                    KnownContents::Unknown
                        if matches!(round, Round::Empty) && woken.woken.load(Ordering::Acquire) =>
                    {
                        round = Round::Woke;
                    }
                    KnownContents::Unknown | KnownContents::Empty => {}
                }
            }
        }
    }
    round
}

/// How many more times, on one read of the drain, an ingress bridge polls its edges while the
/// receive of an endpoint that does not know whether its edge holds anything answers `Pending`
/// and wakes the bridge's task in that poll. A receive that yields to its executor before it
/// hands over an envelope it holds does so for a poll or two; a receive that waits on an empty,
/// open edge by waking itself does so on every poll, and the parent's senders are not gone at the
/// drain, so without a bound such an edge would hold the node's drain until a cancel, the bridge
/// polling it meanwhile. Small, since each round polls every edge.
const DRAIN_REPOLLS: usize = 4;

/// The waker an ingress bridge polls an edge with while the node drains: it wakes the bridge's
/// task and records that it was woken, so that a receive that answered `Pending` and woke its own
/// task in the same poll, asking to be polled again, is not taken for an empty edge.
struct Woken {
    woken: AtomicBool,
    task: Waker,
}

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::Release);
        self.task.wake_by_ref();
    }
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
///
/// The bridge reads whether the node drains before it polls its edges, and ends for the drain only
/// when every edge was then empty. The parent drains the node only once everything it admitted
/// before is on the node's edges, so an edge found empty after the drain was read holds nothing
/// more. Read the other way round, an envelope the parent put on an edge between the bridge's poll
/// of it and the read of the drain would stay on the edge and be dropped with it
/// (`story:subgraph-nested-drain-residual-loss`).
///
/// At its first read of the drain the bridge closes every parent edge it holds as known, with no
/// `recv` future of it alive, to that edge's senders ([`InputEndpoint::close_to_senders`]), in
/// one step that also says what the edge still holds. A send into the edge that has not landed,
/// such as one the parent's ingress admitted before the drain and that still waits for room, is
/// then refused as closed with its envelope instead of landing on an edge nobody reads, and the
/// bridge never waits for its caller to poll it
/// (`story:run-ingress-admitted-send-outlives-the-drain`). An edge closed holding nothing ends for
/// the drain, and one closed holding something is received until its receive answers `None`. An
/// edge whose receive stays in flight, and one whose endpoint does not close, are read as follows;
/// the parent run's edge from another of its nodes is one that does not close, since a node on a
/// feedback cycle through this one drains with it and may still send.
///
/// The senders of the parent's edges are not gone when it drains the node, so the bridge cannot
/// wait for a receive to answer `None` (`story:drain-treats-a-pending-receive-as-an-empty-edge`).
/// While the node drains, an edge whose receive answered `Pending` is read by what its endpoint
/// says it holds ([`InputEndpoint::known_contents`]), which the bridge asks between receives:
/// one that holds an envelope is waited for until its receive hands it over, and one that holds
/// nothing is empty. An edge whose endpoint does not know is read as empty, except that a receive
/// that answers `Pending` and wakes the bridge's task in the same poll has asked to be polled
/// again: on each read of the drain the bridge polls its edges again, up to [`DRAIN_REPOLLS`]
/// more times, while such a receive does so. What the bridge cannot see is an endpoint that does
/// not know, whose receive holds an envelope and answers `Pending` without waking it, to be woken
/// later from another thread: the bridge ends for the drain and that envelope stays on the edge.
async fn ingress_bridge<P: From<ControlKind> + Send + 'static>(
    parents: Vec<Box<dyn InputEndpoint<P>>>,
    ingress: Box<dyn OutputEndpoint<P>>,
    room: Option<Room<P>>,
    open: OpenIngress,
    crossing: Crossing,
) {
    let bridges = &open.0;
    let mut pending: Vec<ParentEdge<P>> = parents.into_iter().map(parent_edge).collect();
    let mut closed = false;
    let mut closed_to_senders = false;
    let mut ended_for_the_drain = false;
    loop {
        if pending.is_empty() {
            if !closed || ended_for_the_drain {
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
            let draining = bridges
                .read(cx, |state| state.drain.then_some(()))
                .is_some();
            if !draining {
                return next_received(&mut pending, cx).map_or(Poll::Pending, Poll::Ready);
            }
            if !std::mem::replace(&mut closed_to_senders, true)
                && close_known_to_senders(&mut pending)
            {
                ended_for_the_drain = true;
            }
            for _ in 0..=DRAIN_REPOLLS {
                match drain_round(&mut pending, cx) {
                    Round::Received(merged) => return Poll::Ready(merged),
                    Round::Holding => return Poll::Pending,
                    Round::Woke => {}
                    Round::Empty => break,
                }
            }
            Poll::Ready(Merged::Drain)
        })
        .await;
        match next {
            Merged::Received(input, Some(envelope), closed_edge) => {
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
                    Sent::Taken if closed_edge => pending.push(ParentEdge::ClosedToSenders(input)),
                    Sent::Taken => pending.push(parent_edge(input)),
                    Sent::Refused | Sent::Cancelled => return,
                }
            }
            Merged::Received(_, None, true) => ended_for_the_drain = true,
            Merged::Received(_, None, false) => closed = true,
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

/// A child egress an egress bridge reads, whether it is a usage egress
/// ([`Declaration::is_usage_egress`]), and whether it is the sink of a child `usage` port no edge
/// leaves and no exposure carries ([`crate::usage_sink`]), whose `stop` the bridge neither waits
/// for nor forwards, since no exposure of the child ends there.
struct ChildEgress<P> {
    endpoint: Box<dyn InputEndpoint<P>>,
    usage: bool,
    sink: bool,
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
    /// The child run has returned and no egress holds anything more.
    ChildEnded,
}

/// Forwards what the child sends out of `egresses` into every parent edge leaving the subgraph
/// node's output port, each copy a crossing of its own, until the child's end of every egress is
/// gone, every parent edge refused, or the child run has returned and no egress holds anything
/// more. An edge that refuses is sent nothing more; the others go on. Once every edge refused,
/// outside a cancel, the bridge records its port for the node's drain to fail. Given no parent
/// edge, the bridge reads the same way and drops what it reads, a cancel ending it as below.
///
/// The child run's return bounds the bridge because a node of the child may keep its egress end
/// past its stop, which `NodeImplementation::stop` does not allow but a run of its own tolerates:
/// once the child run has returned its nodes send nothing more, so the bridge takes what the
/// egresses already hold, without waiting, and ends instead of holding the node's drain.
///
/// A cancel of the node ends the bridge at once, forwarding nothing more, unless it reads a usage
/// egress. Usage is never lost to a cancel: a model node sends the record of the turn the cancel
/// cut after its own cancel (`story:llm-usage-on-cancel`), so at the cancel the bridge stops
/// reading every other egress and goes on reading its usage egresses until the child run is
/// terminal and they close, and its sends of their envelopes do not give way to the cancel: each
/// waits until the parent edge takes it or refuses it, closed, or until [`USAGE_STOP_GRACE`]
/// after the node's stop (at once after its drop). A send given up so ends the forwarding: the
/// bridge goes on reading its egresses, so the child's nodes are not refused, and counts each
/// usage record it reads then, and the one given up, as dropped.
///
/// `egresses` is the child's egress of the port's name, or, for the node's usage port, that and
/// every usage egress the port forwards ([`Declaration::forwarded_usage`]), merged as they arrive,
/// so every usage record of the child crosses once, with the sink of every child `usage` port no
/// edge leaves and no exposure carries ([`crate::usage_sink`]). A `stop` ends one egress's input:
/// it crosses once every egress has sent one, the last to arrive, and none crosses if an egress
/// ended without it, as on the ingress side; so the parent sees one end of input whatever it
/// merges. A sink's `stop` is not waited for and does not cross, since it ends no exposure.
async fn egress_bridge<P: Clone + Send + 'static>(
    egresses: Vec<ChildEgress<P>>,
    parents: Vec<Box<dyn OutputEndpoint<P>>>,
    bridges: Arc<Bridges>,
    crossing: Crossing,
) {
    let mut parents = parents;
    let discards = parents.is_empty();
    let sinks: Vec<bool> = egresses.iter().map(|egress| egress.sink).collect();
    let mut stopped = sinks.clone();
    let usage: Vec<bool> = egresses.iter().map(|egress| egress.usage).collect();
    let mut cancelled = false;
    let mut abandoned = false;
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
            bridges
                .read(cx, |state| state.child_ended.then_some(()))
                .map_or(Poll::Pending, |()| Poll::Ready(Next::ChildEnded))
        })
        .await;
        let (index, egress, received) = match next {
            Next::Cancelled => {
                cancelled = true;
                pending.retain(|(index, _)| usage[*index]);
                continue;
            }
            Next::ChildEnded => return,
            Next::Received(index, egress, received) => (index, egress, received),
        };
        let Some(envelope) = received else {
            continue;
        };
        pending.push((index, indexed(index, egress)));
        if envelope.kind == ControlKind::Stop.as_str() {
            if sinks[index] {
                continue;
            }
            stopped[index] = true;
            if !stopped.iter().all(|stopped| *stopped) {
                continue;
            }
        }
        let record = usage[index] && envelope.kind != ControlKind::Stop.as_str();
        if abandoned {
            if record {
                bridges.update(|state| state.dropped_usage += 1);
            }
            continue;
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
                Sent::Cancelled if usage[index] => {
                    abandoned = true;
                    break;
                }
                Sent::Cancelled => return,
            }
        }
        if abandoned {
            if record {
                bridges.update(|state| state.dropped_usage += 1);
            }
            parents = Vec::new();
            continue;
        }
        if kept.is_empty() && !discards {
            bridges.update(|state| {
                if !state.cancel && state.refused.is_none() {
                    state.refused = Some(crossing.port.clone());
                }
            });
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

/// Polls `future` on this thread until it completes, parking between polls until a wake or the
/// earliest instant a task waits for on the timers of `bridges`, whose tasks it wakes once their
/// instant has passed.
fn block_on<F: Future>(future: F, bridges: &Bridges) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        match bridges.fire_due() {
            Some(at) => thread::park_timeout(at.saturating_duration_since(Instant::now())),
            None => thread::park(),
        }
    }
}
