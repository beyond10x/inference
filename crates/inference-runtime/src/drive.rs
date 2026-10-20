//! What the run loop of [`RunController::drive`](crate::RunController::drive) waits on, and what
//! the run checks on every envelope a node emits.
//!
//! The loop waits on the end of input of every source node and on the requests its caller makes
//! through [`RunRequests`]. A source node's end of input is the control event of kind `stop`
//! ([`ControlKind::Stop`]) it sends on each of its output edges: the source has ended once every
//! edge it feeds has taken that event, and the run's input has ended once every source that feeds
//! an edge has ended. The caller of a run whose pipeline exposes an input port is the source of
//! that ingress (`inference.graph.Exposure`): its end of input is the `stop` it sends on the
//! ingress, which counts as a source edge's. Two gaps follow from reading the end of input off the
//! edges and nothing else:
//!
//! - a source or caller that drops its senders without sending `stop` has not ended its input, so
//!   the loop never drains its run on its own; only a request ends it;
//! - a `stop` the edge took counts as sent even when the edge's overload policy then discarded
//!   it (`drop`, `coalesce`, `drop_stale_epochs`), so the loop may drain a run whose consumer
//!   never sees the end of input.
//!
//! The loop also wakes when a node's exchange ended while the run is Active and a producer still
//! sends to it: the edge into the node refused an envelope as closed ([`SendError::Closed`]),
//! its receiver gone with the exchange. That is how a consumer whose exchange failed shows, and
//! its producers' end of input, refused the same way, would never end the run's input; the loop
//! takes the node's drain at once and, when it answers an error, fails the node and with it the
//! run ([`RunController::fail_node`](crate::RunController::fail_node)). While the run is Draining
//! the refusal is kept too: a producer whose drain then answers the closed edge as its error is
//! not the node that failed, so the drain asks the node the edge arrives at for its own answer
//! first and fails the run naming it with its own error.
//!
//! Every sender the run controller hands a node, and the sender of every ingress it hands its
//! caller, is a [`GuardedSender`]: it refuses an envelope
//! of user content whose `sequence` is not greater than the last one that edge took on the same
//! `stream_id` (`docs/design/inference.md:312` and §23.1 test 6), so a repeated or decreasing
//! sequence never reaches the edge, and the run records the refusal
//! ([`SequenceNotIncreasing`]). A control event, told from user content by its `kind` alone, is
//! not checked: like the event log of the testkit, which records `inference.stream` events only,
//! the check orders the content of a stream. The refused send answers
//! [`SendError::SequenceNotIncreasing`], and the run records the refusal, read from
//! [`RunController::refusals`](crate::RunController::refusals).

use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::fmt;
use std::future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

use inference_core::endpoint::{EndpointFuture, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, is_control_kind};
use inference_core::id::{NodeId, StreamId};
use inference_pipeline::ExposureId;

use crate::queue::Channel;
use crate::run::{FailureReason, RunState};

/// What feeds the run's input: a source node, over each of its output edges, or the caller, over
/// an ingress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Feed {
    Node(NodeId),
    Ingress(ExposureId),
}

/// What the run loop acts on next while the run is Active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Next {
    /// The caller asked for the run to end before it completes.
    Halt(Halt),
    /// `DrainRun`: the caller asked for it, or every source ended its input.
    Drain,
    /// An edge into this node refused an envelope as closed while the run was Active.
    ConsumerGone(NodeId),
}

/// How the caller asked for the run to end before it completes; a failure comes before a cancel
/// when both were asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Halt {
    /// `FailRun` with this reason.
    Fail(FailureReason),
    /// `CancelRun`.
    Cancel,
}

/// A node, or the caller on an ingress, sent an envelope of user content whose `sequence` is not
/// greater than `last`, the last one the edge or exposure `channel` took on `stream_id`; the
/// envelope did not reach its queue. `producer` is the envelope's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceNotIncreasing {
    pub channel: Channel,
    pub producer: NodeId,
    pub stream_id: StreamId,
    pub last: i64,
    pub sequence: i64,
}

impl fmt::Display for SequenceNotIncreasing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "node {} sent sequence {} on the stream {} along {}, not greater than its last, {}",
            self.producer, self.sequence, self.stream_id, self.channel, self.last
        )
    }
}

impl Error for SequenceNotIncreasing {}

struct State {
    run: RunState,
    /// The output edges of each source node, and each ingress, that have not yet taken its end of
    /// input.
    open_sources: HashMap<Feed, usize>,
    /// Whether any source node feeds an edge, or the caller an ingress.
    has_sources: bool,
    /// The first failure the caller reported and the loop has not acted on.
    fail: Option<FailureReason>,
    cancel: bool,
    drain: bool,
    /// The nodes an edge into which refused an envelope as closed while the run was Active, in
    /// the order the refusals happened.
    gone: VecDeque<NodeId>,
    refusals: Vec<SequenceNotIncreasing>,
    waker: Option<Waker>,
}

impl State {
    fn wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    fn input_ended(&self) -> bool {
        self.has_sources && self.open_sources.values().all(|open| *open == 0)
    }

    /// Takes the failure the caller reported, else the cancel it asked for. A failure taken
    /// leaves a cancel asked for, which the failed run then drops.
    fn take_halt(&mut self) -> Option<Halt> {
        if let Some(failure) = self.fail.take() {
            return Some(Halt::Fail(failure));
        }
        std::mem::take(&mut self.cancel).then_some(Halt::Cancel)
    }
}

/// The signals of one run, shared by its controller, the senders of its edges and the
/// [`RunRequests`] its caller holds.
pub(crate) struct Signals {
    state: Mutex<State>,
}

impl Default for Signals {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                run: RunState::Created,
                open_sources: HashMap::new(),
                has_sources: false,
                fail: None,
                cancel: false,
                drain: false,
                gone: VecDeque::new(),
                refusals: Vec::new(),
                waker: None,
            }),
        }
    }
}

impl Signals {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The run is now `run`: a drain asked for is dropped once the run left Active, a node an edge
    /// into which refused an envelope as closed once it left Active and Draining, and every
    /// request once it is terminal.
    pub(crate) fn run_moved(&self, run: RunState) {
        let mut state = self.state();
        state.run = run;
        if run != RunState::Active {
            state.drain = false;
        }
        if !matches!(run, RunState::Active | RunState::Draining) {
            state.gone.clear();
        }
        if run.is_terminal() {
            state.fail = None;
            state.cancel = false;
        }
    }

    /// Counts one more output edge of a source node, or one more ingress.
    pub(crate) fn source_edge(&self, feed: Feed) {
        let mut state = self.state();
        state.has_sources = true;
        *state.open_sources.entry(feed).or_default() += 1;
    }

    /// One output edge of a source node, or an ingress, took its end of input.
    fn source_edge_ended(&self, feed: Feed) {
        let mut state = self.state();
        if let Some(open) = state.open_sources.get_mut(&feed) {
            *open = open.saturating_sub(1);
        }
        if state.input_ended() {
            state.wake();
        }
    }

    /// An edge into the node `node_id` refused an envelope as closed: kept while the run is
    /// Active, for the loop to act on, and while it is Draining, for the drain to name the node
    /// whose exchange failed rather than the producer the closed edge then failed.
    fn consumer_gone(&self, node_id: NodeId) {
        let mut state = self.state();
        if matches!(state.run, RunState::Active | RunState::Draining)
            && !state.gone.contains(&node_id)
        {
            state.gone.push_back(node_id);
            state.wake();
        }
    }

    /// Takes every node an edge into which refused an envelope as closed and that the loop has
    /// not acted on, in the order they were recorded.
    pub(crate) fn take_gone(&self) -> Vec<NodeId> {
        std::mem::take(&mut self.state().gone).into()
    }

    fn refused(&self, refusal: SequenceNotIncreasing) {
        self.state().refusals.push(refusal);
    }

    /// Every envelope the run refused, in the order it refused them.
    pub(crate) fn refusals(&self) -> Vec<SequenceNotIncreasing> {
        self.state().refusals.clone()
    }

    /// What to do next from Active, once there is something: a failure the caller reported or a
    /// cancel it asked for comes first, the failure before the cancel, then a drain it asked for
    /// or the end of every source's input, then a node an edge into which refused an envelope as
    /// closed. Each is taken once.
    pub(crate) async fn next_from_active(&self) -> Next {
        future::poll_fn(|cx| {
            let mut state = self.state();
            if let Some(halt) = state.take_halt() {
                return Poll::Ready(Next::Halt(halt));
            }
            if std::mem::take(&mut state.drain) || state.input_ended() {
                return Poll::Ready(Next::Drain);
            }
            if let Some(node_id) = state.gone.pop_front() {
                return Poll::Ready(Next::ConsumerGone(node_id));
            }
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    /// Takes a failure the caller reported or a cancel it asked for, if it did; otherwise the
    /// task of `cx` is woken by the next request.
    pub(crate) fn poll_halt(&self, cx: &Context<'_>) -> Option<Halt> {
        let mut state = self.state();
        let halt = state.take_halt();
        if halt.is_none() {
            state.waker = Some(cx.waker().clone());
        }
        halt
    }

    /// Takes a failure the caller reported or a cancel it asked for, if it did.
    pub(crate) fn take_halt(&self) -> Option<Halt> {
        self.state().take_halt()
    }

    fn request(&self, request: Next) {
        let mut state = self.state();
        match request {
            Next::Halt(Halt::Fail(failure)) if !state.run.is_terminal() => {
                state.fail.get_or_insert(failure);
            }
            Next::Halt(Halt::Cancel) if !state.run.is_terminal() => state.cancel = true,
            Next::Drain if state.run == RunState::Active => state.drain = true,
            Next::Halt(_) | Next::Drain | Next::ConsumerGone(_) => return,
        }
        state.wake();
    }
}

/// How the caller of [`RunController::drive`](crate::RunController::drive) asks the loop to
/// cancel, drain or fail the run while it runs; clones ask the same loop. A request the run's
/// state machine does not admit when it is made is dropped, and so is one the run has left
/// behind: a drain is kept only while the run is Active, a cancel or a failure only until the run
/// is terminal. So the loop issues no command the run would refuse.
#[derive(Clone)]
pub struct RunRequests {
    pub(crate) signals: Arc<Signals>,
}

impl RunRequests {
    /// Asks for `inference.graph.CancelRun`; dropped once the run is terminal.
    pub fn cancel(&self) {
        self.signals.request(Next::Halt(Halt::Cancel));
    }

    /// Asks for `inference.graph.DrainRun`; dropped unless the run is Active.
    pub fn drain(&self) {
        self.signals.request(Next::Drain);
    }

    /// Asks for `inference.graph.FailRun` with `failure`, which names what failed: something the
    /// caller watches, such as the egress watch of the `egress` module on Linux, rather than a node
    /// the run drives. Dropped once the run is terminal; of two failures asked for before the
    /// loop takes one, the first is kept. A failure is taken before a cancel.
    pub fn fail(&self, failure: FailureReason) {
        self.signals.request(Next::Halt(Halt::Fail(failure)));
    }
}

impl fmt::Debug for RunRequests {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunRequests").finish_non_exhaustive()
    }
}

/// The sender of one output edge as the run controller hands it to a node, or of one exposure as
/// it hands it to a node (an egress) or to its caller (an ingress): it refuses user content whose
/// sequence does not increase on its stream, tells the run's [`Signals`] when the queue refuses an
/// envelope as closed, naming the node it arrives at, and, on an edge of a source node or an
/// ingress, once the queue took the end of input.
pub(crate) struct GuardedSender<E> {
    pub(crate) inner: E,
    pub(crate) channel: Channel,
    /// What the queue feeds the run's input from, if it is a source node's edge or an ingress.
    pub(crate) source: Option<Feed>,
    /// The node the queue arrives at, if it arrives at one.
    pub(crate) consumer: Option<NodeId>,
    pub(crate) signals: Arc<Signals>,
    pub(crate) ended: AtomicBool,
    /// The last sequence the queue took on each stream.
    pub(crate) sequences: Mutex<HashMap<StreamId, i64>>,
}

impl<E> GuardedSender<E> {
    pub(crate) fn new(
        inner: E,
        channel: Channel,
        source: Option<Feed>,
        consumer: Option<NodeId>,
        signals: Arc<Signals>,
    ) -> Self {
        Self {
            inner,
            channel,
            source,
            consumer,
            signals,
            ended: AtomicBool::new(false),
            sequences: Mutex::new(HashMap::new()),
        }
    }

    fn sequences(&self) -> MutexGuard<'_, HashMap<StreamId, i64>> {
        self.sequences
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes `sequence` on `stream_id` as the edge's last and answers the one it replaced, or
    /// refuses it when it is not greater than the last.
    fn advance(&self, stream_id: StreamId, sequence: i64) -> Result<Option<i64>, i64> {
        let mut sequences = self.sequences();
        match sequences.get(&stream_id) {
            Some(&last) if sequence <= last => Err(last),
            previous => {
                let previous = previous.copied();
                sequences.insert(stream_id, sequence);
                Ok(previous)
            }
        }
    }

    /// Gives `sequence` back after the edge refused its envelope, unless a later one was taken.
    fn restore(&self, stream_id: StreamId, sequence: i64, previous: Option<i64>) {
        let mut sequences = self.sequences();
        if sequences.get(&stream_id) == Some(&sequence) {
            match previous {
                Some(previous) => sequences.insert(stream_id, previous),
                None => sequences.remove(&stream_id),
            };
        }
    }
}

impl<P: Send + 'static, E: OutputEndpoint<P>> OutputEndpoint<P> for GuardedSender<E> {
    fn send(&self, envelope: EventEnvelope<P>) -> EndpointFuture<'_, Result<(), SendError<P>>> {
        let end = self.source.is_some() && envelope.kind == ControlKind::Stop.as_str();
        let checked = (!is_control_kind(&envelope.kind)).then_some((
            envelope.stream_id,
            envelope.sequence,
            envelope.producer,
        ));
        Box::pin(async move {
            let mut taken = None;
            if let Some((stream_id, sequence, producer)) = checked {
                match self.advance(stream_id, sequence) {
                    Ok(previous) => taken = Some((stream_id, sequence, previous)),
                    Err(last) => {
                        self.signals.refused(SequenceNotIncreasing {
                            channel: self.channel,
                            producer,
                            stream_id,
                            last,
                            sequence,
                        });
                        return Err(SendError::SequenceNotIncreasing { envelope, last });
                    }
                }
            }
            if let Err(refused) = self.inner.send(envelope).await {
                if let Some((stream_id, sequence, previous)) = taken {
                    self.restore(stream_id, sequence, previous);
                }
                if let (SendError::Closed(_), Some(node_id)) = (&refused, self.consumer) {
                    self.signals.consumer_gone(node_id);
                }
                return Err(refused);
            }
            if end
                && !self.ended.swap(true, Ordering::AcqRel)
                && let Some(feed) = self.source
            {
                self.signals.source_edge_ended(feed);
            }
            Ok(())
        })
    }
}
