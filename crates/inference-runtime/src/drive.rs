//! What the run loop of [`RunController::drive`](crate::RunController::drive) waits on, and what
//! the run checks on every envelope a node emits.
//!
//! The loop waits on the end of input of every source node and on the requests its caller makes
//! through [`RunRequests`]. A source node's end of input is the control event of kind `stop`
//! ([`ControlKind::Stop`]) it sends on each of its output edges: the source has ended once every
//! edge it feeds has taken that event, and the run's input has ended once every source that feeds
//! an edge has ended. Two gaps follow from reading the end of input off the edges and nothing
//! else:
//!
//! - a source that drops its senders without sending `stop` has not ended its input, so the loop
//!   never drains its run on its own; only a request ends it;
//! - a `stop` the edge took counts as sent even when the edge's overload policy then discarded
//!   it (`drop`, `coalesce`, `drop_stale_epochs`), so the loop may drain a run whose consumer
//!   never sees the end of input.
//!
//! Every sender the run controller hands a node is a [`GuardedSender`]: it refuses an envelope
//! of user content whose `sequence` is not greater than the last one that edge took on the same
//! `stream_id` (`docs/design/inference.md:312` and §23.1 test 6), so a repeated or decreasing
//! sequence never reaches the edge, and the run records the refusal
//! ([`SequenceNotIncreasing`]). A control event, told from user content by its `kind` alone, is
//! not checked: like the event log of the testkit, which records `inference.stream` events only,
//! the check orders the content of a stream. The refused send answers
//! [`SendError::SequenceNotIncreasing`], and the run records the refusal, read from
//! [`RunController::refusals`](crate::RunController::refusals).

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

use inference_core::endpoint::{EndpointFuture, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, is_control_kind};
use inference_core::id::{NodeId, StreamId};
use inference_pipeline::EdgeId;

use crate::run::RunState;

/// What the run loop acts on next while the run is Active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Next {
    /// `CancelRun`: the caller asked for it.
    Cancel,
    /// `DrainRun`: the caller asked for it, or every source ended its input.
    Drain,
}

/// A node sent an envelope of user content whose `sequence` is not greater than `last`, the
/// last one the edge `edge_id` took on `stream_id`; the envelope did not reach the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequenceNotIncreasing {
    pub edge_id: EdgeId,
    pub producer: NodeId,
    pub stream_id: StreamId,
    pub last: i64,
    pub sequence: i64,
}

impl fmt::Display for SequenceNotIncreasing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "node {} sent sequence {} on the stream {} along edge {}, not greater than its last, {}",
            self.producer, self.sequence, self.stream_id, self.edge_id, self.last
        )
    }
}

impl Error for SequenceNotIncreasing {}

struct State {
    run: RunState,
    /// The output edges of each source node that have not yet taken its end of input.
    open_sources: HashMap<NodeId, usize>,
    /// Whether any source node feeds an edge.
    has_sources: bool,
    cancel: bool,
    drain: bool,
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
                cancel: false,
                drain: false,
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

    /// The run is now `run`: a drain asked for is dropped once the run left Active, and every
    /// request once it is terminal.
    pub(crate) fn run_moved(&self, run: RunState) {
        let mut state = self.state();
        state.run = run;
        if run != RunState::Active {
            state.drain = false;
        }
        if run.is_terminal() {
            state.cancel = false;
        }
    }

    /// Counts one more output edge of the source node `node_id`.
    pub(crate) fn source_edge(&self, node_id: NodeId) {
        let mut state = self.state();
        state.has_sources = true;
        *state.open_sources.entry(node_id).or_default() += 1;
    }

    /// One output edge of the source node `node_id` took its end of input.
    fn source_edge_ended(&self, node_id: NodeId) {
        let mut state = self.state();
        if let Some(open) = state.open_sources.get_mut(&node_id) {
            *open = open.saturating_sub(1);
        }
        if state.input_ended() {
            state.wake();
        }
    }

    fn refused(&self, refusal: SequenceNotIncreasing) {
        self.state().refusals.push(refusal);
    }

    /// Every envelope the run refused, in the order it refused them.
    pub(crate) fn refusals(&self) -> Vec<SequenceNotIncreasing> {
        self.state().refusals.clone()
    }

    /// What to do next from Active, once there is something: a cancel the caller asked for comes
    /// first, then a drain it asked for or the end of every source's input. Each request is
    /// taken once.
    pub(crate) async fn next_from_active(&self) -> Next {
        future::poll_fn(|cx| {
            let mut state = self.state();
            if std::mem::take(&mut state.cancel) {
                return Poll::Ready(Next::Cancel);
            }
            if std::mem::take(&mut state.drain) || state.input_ended() {
                return Poll::Ready(Next::Drain);
            }
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    /// Takes a cancel the caller asked for, if it did; otherwise the task of `cx` is woken by the
    /// next request.
    pub(crate) fn poll_cancel(&self, cx: &Context<'_>) -> bool {
        let mut state = self.state();
        if std::mem::take(&mut state.cancel) {
            return true;
        }
        state.waker = Some(cx.waker().clone());
        false
    }

    /// Takes a cancel the caller asked for, if it did.
    pub(crate) fn take_cancel(&self) -> bool {
        std::mem::take(&mut self.state().cancel)
    }

    fn request(&self, request: Next) {
        let mut state = self.state();
        match request {
            Next::Cancel if !state.run.is_terminal() => state.cancel = true,
            Next::Drain if state.run == RunState::Active => state.drain = true,
            Next::Cancel | Next::Drain => return,
        }
        state.wake();
    }
}

/// How the caller of [`RunController::drive`](crate::RunController::drive) asks the loop to
/// cancel or drain the run while it runs; clones ask the same loop. A request the run's state
/// machine does not admit when it is made is dropped, and so is one the run has left behind: a
/// drain is kept only while the run is Active, a cancel only until the run is terminal. So the
/// loop issues no command the run would refuse.
#[derive(Clone)]
pub struct RunRequests {
    pub(crate) signals: Arc<Signals>,
}

impl RunRequests {
    /// Asks for `inference.graph.CancelRun`; dropped once the run is terminal.
    pub fn cancel(&self) {
        self.signals.request(Next::Cancel);
    }

    /// Asks for `inference.graph.DrainRun`; dropped unless the run is Active.
    pub fn drain(&self) {
        self.signals.request(Next::Drain);
    }
}

impl fmt::Debug for RunRequests {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunRequests").finish_non_exhaustive()
    }
}

/// The sender of one output edge as the run controller hands it to a node: it refuses user
/// content whose sequence does not increase on its stream, and, on an edge of a source node,
/// tells the run's [`Signals`] once the edge took the node's end of input.
pub(crate) struct GuardedSender<E> {
    pub(crate) inner: E,
    pub(crate) edge_id: EdgeId,
    /// The source node the edge leaves from, if it leaves from one.
    pub(crate) source: Option<NodeId>,
    pub(crate) signals: Arc<Signals>,
    pub(crate) ended: AtomicBool,
    /// The last sequence the edge took on each stream.
    pub(crate) sequences: Mutex<HashMap<StreamId, i64>>,
}

impl<E> GuardedSender<E> {
    pub(crate) fn new(
        inner: E,
        edge_id: EdgeId,
        source: Option<NodeId>,
        signals: Arc<Signals>,
    ) -> Self {
        Self {
            inner,
            edge_id,
            source,
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
                            edge_id: self.edge_id,
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
                return Err(refused);
            }
            if end
                && !self.ended.swap(true, Ordering::AcqRel)
                && let Some(node_id) = self.source
            {
                self.signals.source_edge_ended(node_id);
            }
            Ok(())
        })
    }
}
