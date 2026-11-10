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
//!
//! `inference.graph.DrainRun` admits no new input; in-flight events complete. So the sender of an
//! ingress refuses an envelope of user content once a drain was asked for while the run was Active
//! ([`RunRequests::drain`]), or once the run moved to Draining, whichever came first, read when
//! the send's future is first polled, so a send built earlier and awaited after the request is
//! refused too: the envelope never reaches the ingress's queue and the send answers
//! [`SendError::Closed`], since
//! nothing of user content sent there is ever delivered again. What the queue took before is in
//! flight and is still delivered, and a subgraph node's ingress bridge forwards what its parent
//! edges hold before it ends (`subgraph` module). A control event still passes, `stop` among
//! them: it carries no input, and the drain of a node that reads the ingress ends on it. The
//! refusal is the guard's own and not the queue's, so it does not tell the run the consumer is
//! gone. The guard does not close the edges between nodes: what a node emits while it drains is
//! in flight.
//!
//! A send admitted before the drain may still wait for room when the drain runs. The run's own
//! ingress, read by a plain node, delivers it once the node takes. A subgraph node's ingress
//! bridge closes the run's ingress to its senders at the drain, so such a send is refused as
//! closed with its envelope rather than landing on an edge nobody reads; that refusal does not
//! tell the run the consumer is gone either. An edge between two nodes is never closed that way:
//! a node on a feedback cycle through the subgraph node drains with it and may still send.

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

use crate::queue::{Channel, QueueSender};
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

/// How many refusals a run keeps ([`RunController::refusals`](crate::RunController::refusals)): the
/// first ones, in the order the run refused them. Each later one is counted
/// ([`RunController::refusals_overflow`](crate::RunController::refusals_overflow)) and not kept, so
/// a node that keeps sending non-increasing sequences does not grow the run without bound.
pub const REFUSALS_KEPT: usize = 256;

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
    /// The run admits no new input: a drain was asked for while it was Active, or it moved to
    /// Draining. Never cleared, since a run does not leave Draining for Active.
    input_closed: bool,
    /// The nodes an edge into which refused an envelope as closed while the run was Active, in
    /// the order the refusals happened.
    gone: VecDeque<NodeId>,
    /// The first [`REFUSALS_KEPT`] refusals, and how many came after them.
    refusals: Vec<SequenceNotIncreasing>,
    refusals_overflow: u64,
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
                input_closed: false,
                gone: VecDeque::new(),
                refusals: Vec::new(),
                refusals_overflow: 0,
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
    /// request once it is terminal. A run that is Draining admits no new input.
    pub(crate) fn run_moved(&self, run: RunState) {
        let mut state = self.state();
        state.run = run;
        if run == RunState::Draining {
            state.input_closed = true;
        }
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

    /// Whether the run still admits new input on its ingresses: not once a drain was asked for
    /// while it was Active, nor once it moved to Draining.
    fn admits_input(&self) -> bool {
        !self.state().input_closed
    }

    /// Keeps `refusal` while fewer than [`REFUSALS_KEPT`] are kept, else counts it.
    fn refused(&self, refusal: SequenceNotIncreasing) {
        let mut state = self.state();
        if state.refusals.len() < REFUSALS_KEPT {
            state.refusals.push(refusal);
        } else {
            state.refusals_overflow = state.refusals_overflow.saturating_add(1);
        }
    }

    /// The first [`REFUSALS_KEPT`] envelopes the run refused, in the order it refused them.
    pub(crate) fn refusals(&self) -> Vec<SequenceNotIncreasing> {
        self.state().refusals.clone()
    }

    /// How many refusals came after the first [`REFUSALS_KEPT`] and were not kept.
    pub(crate) fn refusals_overflow(&self) -> u64 {
        self.state().refusals_overflow
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
            Next::Drain if state.run == RunState::Active => {
                state.drain = true;
                state.input_closed = true;
            }
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

    /// Asks for `inference.graph.DrainRun`; dropped unless the run is Active. A drain taken closes
    /// the run's ingresses to new input at once, before the loop issues `DrainRun`: see the
    /// module documentation.
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
/// ingress, once the queue took the end of input. On an ingress it refuses user content once the
/// run admits no new input, before the queue sees it. A refusal as closed by a queue its reader
/// closed to senders
/// ([`close_to_senders`](inference_core::endpoint::InputEndpoint::close_to_senders)) is a send
/// that was not delivered: its sequence is given back, as for every refusal, and the node is
/// draining, not gone, so the run is not told.
pub(crate) struct GuardedSender<E> {
    pub(crate) inner: E,
    pub(crate) channel: Channel,
    /// What the queue feeds the run's input from, if it is a source node's edge or an ingress.
    pub(crate) source: Option<Feed>,
    /// The node the queue arrives at, if it arrives at one.
    pub(crate) consumer: Option<NodeId>,
    pub(crate) signals: Arc<Signals>,
    pub(crate) ended: AtomicBool,
    /// What the queue took on each stream, and what is on its way into it.
    pub(crate) sequences: Mutex<HashMap<StreamId, Sequences>>,
}

/// The sequences of one stream along one edge: the highest one the queue took, and those of the
/// sends under way, which the queue may still take or which give theirs back. The stream's last
/// is the highest of them all, so a send given up gives back exactly its own sequence, in
/// whatever order sends are given up.
#[derive(Debug, Default)]
pub(crate) struct Sequences {
    taken: Option<i64>,
    under_way: Vec<i64>,
}

impl Sequences {
    fn last(&self) -> Option<i64> {
        self.under_way.iter().copied().chain(self.taken).max()
    }
}

impl<P> GuardedSender<QueueSender<P>> {
    /// The guard of `inner`, the sender of the queue `channel` names. The queue of an edge
    /// between two nodes is kept open to its senders
    /// ([`QueueSender::keep_open_to_senders`]): a node on a feedback cycle through a subgraph
    /// node drains with it and may still send into it, so only a run's exposures, its ingress
    /// among them, are closed by a reader at its drain.
    pub(crate) fn new(
        inner: QueueSender<P>,
        channel: Channel,
        source: Option<Feed>,
        consumer: Option<NodeId>,
        signals: Arc<Signals>,
    ) -> Self {
        if matches!(channel, Channel::Edge(_)) {
            inner.keep_open_to_senders();
        }
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

    fn sequences(&self) -> MutexGuard<'_, HashMap<StreamId, Sequences>> {
        self.sequences
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Puts `sequence` on `stream_id` under way, or refuses it with the stream's last when it is
    /// not greater than that.
    fn advance(&self, stream_id: StreamId, sequence: i64) -> Result<(), i64> {
        let mut sequences = self.sequences();
        let stream = sequences.entry(stream_id).or_default();
        match stream.last() {
            Some(last) if sequence <= last => Err(last),
            _ => {
                stream.under_way.push(sequence);
                Ok(())
            }
        }
    }

    /// The send of `sequence` on `stream_id` ended: the queue took its envelope when `taken`,
    /// and otherwise the sequence is given back.
    fn settle(&self, stream_id: StreamId, sequence: i64, taken: bool) {
        let mut sequences = self.sequences();
        if let Some(stream) = sequences.get_mut(&stream_id) {
            if let Some(at) = stream.under_way.iter().position(|&held| held == sequence) {
                stream.under_way.swap_remove(at);
            }
            if taken {
                stream.taken = stream.taken.max(Some(sequence));
            }
        }
    }
}

/// A send of a [`GuardedSender`] under way, with the sequence it advanced. Dropped before it
/// ended, it gives the sequence back: a send of the queue that has not answered has never pushed
/// its envelope, so the queue took nothing and the same envelope sent again is taken.
struct Underway<'a, P> {
    sender: &'a GuardedSender<QueueSender<P>>,
    checked: Option<(StreamId, i64)>,
}

impl<P> Drop for Underway<'_, P> {
    fn drop(&mut self) {
        if let Some((stream_id, sequence)) = self.checked.take() {
            self.sender.settle(stream_id, sequence, false);
        }
    }
}

impl<P: Send + 'static> OutputEndpoint<P> for GuardedSender<QueueSender<P>> {
    fn send(&self, envelope: EventEnvelope<P>) -> EndpointFuture<'_, Result<(), SendError<P>>> {
        let end = self.source.is_some() && envelope.kind == ControlKind::Stop.as_str();
        let content = !is_control_kind(&envelope.kind);
        let guarded = content && matches!(self.source, Some(Feed::Ingress(_)));
        let checked = content.then_some((envelope.stream_id, envelope.sequence, envelope.producer));
        Box::pin(async move {
            if guarded && !self.signals.admits_input() {
                return Err(SendError::Closed(envelope));
            }
            let mut under_way = None;
            if let Some((stream_id, sequence, producer)) = checked {
                match self.advance(stream_id, sequence) {
                    Ok(()) => under_way = Some((stream_id, sequence)),
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
            let mut underway = Underway {
                sender: self,
                checked: under_way,
            };
            let sent = self.inner.send(envelope).await;
            if let Some((stream_id, sequence)) = underway.checked.take() {
                self.settle(stream_id, sequence, sent.is_ok());
            }
            if let Err(refused) = sent {
                if let (SendError::Closed(_), Some(node_id)) = (&refused, self.consumer)
                    && !self.inner.is_closed_to_senders()
                {
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
