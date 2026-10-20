//! The queue of one `inference.graph.Edge`, or of one `inference.graph.Exposure`, in one run:
//! finite buffering with the overload behaviour the edge or exposure declares
//! (`docs/design/inference.md` §12.3). An exposure's queue behaves as an edge's; its far end is
//! the run's caller, and an [`ExposureWatch`] shows it dropped.
//!
//! An [`EdgeQueue`] is built from an [`Edge`] and holds at most [`Edge::capacity`] envelopes
//! under every policy. Its [`QueueSender`] implements [`OutputEndpoint`] and its
//! [`QueueReceiver`] implements [`InputEndpoint`]; the receiver takes envelopes in arrival order.
//! What a send does when the queue holds `capacity` envelopes is the edge's
//! [`OverloadPolicy`]:
//!
//! - `pause`: the send waits until the consumer takes an envelope; nothing is lost.
//! - `fail`: the send returns [`SendError::Full`] carrying the envelope; nothing is discarded
//!   silently.
//! - `drop`: the arriving envelope is discarded and the producer is not held.
//! - `coalesce`: the arriving envelope replaces, in place, the newest queued envelope of the same
//!   `stream_id`; if no queued envelope carries that stream, the oldest queued envelope is
//!   discarded and the arriving one is queued. The order within each stream is kept.
//! - `drop_stale_epochs`: the queue remembers the highest epoch it has seen per producing node.
//!   An arriving envelope of a lower epoch is discarded whether the queue is full or not; one of
//!   a higher epoch first discards the queued envelopes of lower epochs from the same producer
//!   and wakes the waiting sends, so that a send of that producer's older epoch completes as
//!   discarded without waiting for the consumer. When the queue is still full the send waits, as
//!   under `pause`.
//!
//! The envelope carries no epoch of its own, so the queue reads the epoch of an envelope's
//! `generation_id` through the [`EpochLookup`] it is built with. An envelope without a
//! generation, such as a control event, or with a generation the lookup does not know, is never
//! stale. Every discarded or replaced envelope adds one to [`QueueReceiver::discarded`], and so
//! does every envelope still queued when the receiver is dropped.
//!
//! `cancel` and `supersede` are refused when the queue is built, with [`UnsupportedOverload`]:
//! the design names them without saying what a full queue does under them.
//!
//! Once the receiver is gone every send returns [`SendError::Closed`]; once every sender is gone
//! the receiver yields what is queued and then `None`. Waiting is a [`Future`](std::future::Future)
//! woken through its [`Waker`], so the queue runs under any executor.

use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::fmt;
use std::future;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context, Poll, Waker};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::EventEnvelope;
use inference_core::id::{GenerationId, NodeId};
use inference_pipeline::{Edge, EdgeId, Exposure, ExposureId, OverloadPolicy};

/// Reads the epoch of the `inference.stream.Generation` an identity names, for the policy
/// `drop_stale_epochs`. A run hands the queue the generations its nodes started; a lookup that
/// does not know an identity returns `None`, and the envelope is then never stale.
pub trait EpochLookup: Send + Sync {
    /// The `epoch` of the generation `generation_id` names, if the lookup knows it.
    fn epoch(&self, generation_id: GenerationId) -> Option<i64>;
}

impl EpochLookup for HashMap<GenerationId, i64> {
    fn epoch(&self, generation_id: GenerationId) -> Option<i64> {
        self.get(&generation_id).copied()
    }
}

impl<T: EpochLookup + ?Sized> EpochLookup for Arc<T> {
    fn epoch(&self, generation_id: GenerationId) -> Option<i64> {
        (**self).epoch(generation_id)
    }
}

/// The edge's overload policy is one the queue does not implement: `cancel` or `supersede`.
/// `edge_id` names the edge and `overload` its policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedOverload {
    pub edge_id: EdgeId,
    pub overload: OverloadPolicy,
}

impl fmt::Display for UnsupportedOverload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "edge {} declares the overload policy {:?}, which no edge queue implements",
            self.edge_id, self.overload
        )
    }
}

impl Error for UnsupportedOverload {}

/// The exposure's overload policy is one the queue does not implement: `cancel` or `supersede`.
/// `exposure_id` names the exposure and `overload` its policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedExposureOverload {
    pub exposure_id: ExposureId,
    pub overload: OverloadPolicy,
}

impl fmt::Display for UnsupportedExposureOverload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "exposure {} declares the overload policy {:?}, which no queue implements",
            self.exposure_id, self.overload
        )
    }
}

impl Error for UnsupportedExposureOverload {}

/// What a queue of a run carries for: an edge of the pipeline, or an exposure, whose far end the
/// run's caller holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Edge(EdgeId),
    Exposure(ExposureId),
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Edge(edge_id) => write!(f, "edge {edge_id}"),
            Self::Exposure(exposure_id) => write!(f, "exposure {exposure_id}"),
        }
    }
}

/// The queue of one exposure in one run: its sender side, its receiver side and a weak reference
/// to it.
pub(crate) struct ExposureQueue<P> {
    pub(crate) sender: QueueSender<P>,
    pub(crate) receiver: QueueReceiver<P>,
    pub(crate) watch: ExposureWatch<P>,
    pub(crate) room: Room<P>,
}

/// A weak handle on a queue's room, for the holder of a sender whose full queue answered
/// [`SendError::Full`] under `fail`: it waits for the receiver to take an envelope, without
/// holding the queue alive.
pub(crate) struct Room<P> {
    shared: Weak<Shared<P>>,
}

impl<P> Room<P> {
    /// Ready once the queue has room, or once it is gone or closed, when a send answers at once;
    /// otherwise the task of `cx` is woken when the receiver next takes an envelope.
    pub(crate) fn poll_room(&self, cx: &Context<'_>) -> Poll<()> {
        let Some(shared) = self.shared.upgrade() else {
            return Poll::Ready(());
        };
        let mut state = shared.state();
        if !state.receiver || state.items.len() < shared.capacity.get() {
            return Poll::Ready(());
        }
        register(&mut state.waiting_senders, cx.waker());
        Poll::Pending
    }
}

/// The queue of `exposure` in one run, as [`EdgeQueue::new`] builds an edge's.
pub(crate) fn exposure_queue<P>(
    exposure: &Exposure,
    epochs: impl EpochLookup + 'static,
) -> Result<ExposureQueue<P>, UnsupportedExposureOverload> {
    let shared = Shared::bounded(exposure.capacity(), exposure.overload(), epochs).ok_or(
        UnsupportedExposureOverload {
            exposure_id: exposure.exposure_id(),
            overload: exposure.overload(),
        },
    )?;
    let watch = ExposureWatch {
        exposure_id: exposure.exposure_id(),
        shared: Arc::downgrade(&shared),
    };
    Ok(ExposureQueue {
        sender: QueueSender {
            shared: Arc::clone(&shared),
        },
        room: Room {
            shared: Arc::downgrade(&shared),
        },
        receiver: QueueReceiver { shared },
        watch,
    })
}

/// The overload policies the queue implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Policy {
    Pause,
    Fail,
    Drop,
    Coalesce,
    DropStaleEpochs,
}

/// One queued envelope with the epoch its generation had when it arrived.
struct Queued<P> {
    envelope: EventEnvelope<P>,
    epoch: Option<i64>,
}

struct State<P> {
    items: VecDeque<Queued<P>>,
    discarded: u64,
    newest_epochs: HashMap<NodeId, i64>,
    senders: usize,
    receiver: bool,
    waiting_senders: Vec<Waker>,
    waiting_receiver: Option<Waker>,
}

struct Shared<P> {
    capacity: NonZeroUsize,
    overload: OverloadPolicy,
    policy: Policy,
    epochs: Box<dyn EpochLookup>,
    state: Mutex<State<P>>,
}

/// Wakes every waker in `wakers`.
fn wake(wakers: impl IntoIterator<Item = Waker>) {
    wakers.into_iter().for_each(Waker::wake);
}

/// Adds `waker` to `wakers` unless one there already wakes the same task.
fn register(wakers: &mut Vec<Waker>, waker: &Waker) {
    if !wakers.iter().any(|known| known.will_wake(waker)) {
        wakers.push(waker.clone());
    }
}

impl<P> Shared<P> {
    fn state(&self) -> MutexGuard<'_, State<P>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn len(&self) -> usize {
        self.state().items.len()
    }

    fn discarded(&self) -> u64 {
        self.state().discarded
    }

    /// One attempt to send the envelope in `slot`. A send that must wait leaves the envelope in
    /// `slot` and registers the task; every other outcome takes it.
    fn poll_send(
        &self,
        cx: &Context<'_>,
        slot: &mut Option<EventEnvelope<P>>,
    ) -> Poll<Result<(), SendError<P>>> {
        let Some(envelope) = slot.take() else {
            return Poll::Ready(Ok(()));
        };
        let epoch = match self.policy {
            Policy::DropStaleEpochs => envelope
                .generation_id
                .and_then(|generation_id| self.epochs.epoch(generation_id)),
            _ => None,
        };
        let mut state = self.state();
        if !state.receiver {
            return Poll::Ready(Err(SendError::Closed(envelope)));
        }
        let mut raised = false;
        if let Some(epoch) = epoch {
            let newest = *state
                .newest_epochs
                .entry(envelope.producer)
                .or_insert(epoch);
            if epoch < newest {
                state.discarded += 1;
                return Poll::Ready(Ok(()));
            }
            if epoch > newest {
                state.newest_epochs.insert(envelope.producer, epoch);
                let before = state.items.len();
                let producer = envelope.producer;
                state.items.retain(|queued| {
                    queued.envelope.producer != producer
                        || queued.epoch.is_none_or(|queued| queued >= epoch)
                });
                let stale = before - state.items.len();
                state.discarded += stale as u64;
                raised = true;
            }
        }
        // A raised epoch may have made a waiting send stale, or freed room for one: every
        // waiting sender polls again, and a stale one completes as discarded.
        let senders = if raised {
            std::mem::take(&mut state.waiting_senders)
        } else {
            Vec::new()
        };
        if state.items.len() < self.capacity.get() {
            state.items.push_back(Queued { envelope, epoch });
            let receiver = state.waiting_receiver.take();
            drop(state);
            wake(receiver);
            wake(senders);
            return Poll::Ready(Ok(()));
        }
        let outcome = match self.policy {
            Policy::Pause | Policy::DropStaleEpochs => {
                register(&mut state.waiting_senders, cx.waker());
                *slot = Some(envelope);
                Poll::Pending
            }
            Policy::Fail => Poll::Ready(Err(SendError::Full(envelope))),
            Policy::Drop => {
                state.discarded += 1;
                Poll::Ready(Ok(()))
            }
            Policy::Coalesce => {
                let stream_id = envelope.stream_id;
                let arrival = Queued { envelope, epoch };
                if let Some(newest) = state
                    .items
                    .iter_mut()
                    .rev()
                    .find(|queued| queued.envelope.stream_id == stream_id)
                {
                    *newest = arrival;
                } else {
                    state.items.pop_front();
                    state.items.push_back(arrival);
                }
                state.discarded += 1;
                Poll::Ready(Ok(()))
            }
        };
        drop(state);
        wake(senders);
        outcome
    }

    /// One attempt to take the oldest queued envelope.
    fn poll_recv(&self, cx: &Context<'_>) -> Poll<Option<EventEnvelope<P>>> {
        let mut state = self.state();
        if let Some(queued) = state.items.pop_front() {
            let senders = std::mem::take(&mut state.waiting_senders);
            drop(state);
            wake(senders);
            return Poll::Ready(Some(queued.envelope));
        }
        if state.senders == 0 {
            return Poll::Ready(None);
        }
        state.waiting_receiver = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl<P> Shared<P> {
    /// An empty queue holding at most `capacity` under `overload`; none when the queue does not
    /// implement `overload`.
    fn bounded(
        capacity: NonZeroUsize,
        overload: OverloadPolicy,
        epochs: impl EpochLookup + 'static,
    ) -> Option<Arc<Self>> {
        let policy = match overload {
            OverloadPolicy::Pause => Policy::Pause,
            OverloadPolicy::Fail => Policy::Fail,
            OverloadPolicy::Drop => Policy::Drop,
            OverloadPolicy::Coalesce => Policy::Coalesce,
            OverloadPolicy::DropStaleEpochs => Policy::DropStaleEpochs,
            OverloadPolicy::Cancel | OverloadPolicy::Supersede => return None,
        };
        let state = State {
            items: VecDeque::new(),
            discarded: 0,
            newest_epochs: HashMap::new(),
            senders: 1,
            receiver: true,
            waiting_senders: Vec::new(),
            waiting_receiver: None,
        };
        Some(Arc::new(Self {
            capacity,
            overload,
            policy,
            epochs: Box::new(epochs),
            state: Mutex::new(state),
        }))
    }
}

/// The queue of one edge in one run, before its two sides are handed out.
pub struct EdgeQueue<P> {
    edge_id: EdgeId,
    shared: Arc<Shared<P>>,
}

impl<P> EdgeQueue<P> {
    /// An empty queue for `edge`, holding at most its capacity under its overload policy, that
    /// reads epochs through `epochs` for the policy `drop_stale_epochs`.
    ///
    /// # Errors
    ///
    /// [`UnsupportedOverload`] when the edge's policy is `cancel` or `supersede`.
    pub fn new(
        edge: &Edge,
        epochs: impl EpochLookup + 'static,
    ) -> Result<Self, UnsupportedOverload> {
        let shared = Shared::bounded(edge.capacity(), edge.overload(), epochs).ok_or(
            UnsupportedOverload {
                edge_id: edge.edge_id(),
                overload: edge.overload(),
            },
        )?;
        Ok(Self {
            edge_id: edge.edge_id(),
            shared,
        })
    }

    /// The edge this queue was built for.
    #[must_use]
    pub fn edge_id(&self) -> EdgeId {
        self.edge_id
    }

    /// The most envelopes the queue holds.
    #[must_use]
    pub fn capacity(&self) -> NonZeroUsize {
        self.shared.capacity
    }

    /// What a send does when the queue is full.
    #[must_use]
    pub fn overload(&self) -> OverloadPolicy {
        self.shared.overload
    }

    /// A weak reference to the queue, which keeps nothing alive: it shows whether the queue was
    /// dropped, once every side of it is gone.
    #[must_use]
    pub fn watch(&self) -> QueueWatch<P> {
        QueueWatch {
            edge_id: self.edge_id,
            shared: Arc::downgrade(&self.shared),
        }
    }

    /// The sender side, for the producing node, and the receiver side, for the consuming node.
    #[must_use]
    pub fn split(self) -> (QueueSender<P>, QueueReceiver<P>) {
        (
            QueueSender {
                shared: Arc::clone(&self.shared),
            },
            QueueReceiver {
                shared: self.shared,
            },
        )
    }
}

/// A weak reference to an [`EdgeQueue`]: it keeps neither side alive and tells whether the queue
/// was dropped, which it is once its senders and its receiver are gone (§23.3 test 7 of
/// `docs/design/inference.md`: graph runs release their resources).
pub struct QueueWatch<P> {
    edge_id: EdgeId,
    shared: Weak<Shared<P>>,
}

impl<P> QueueWatch<P> {
    /// The edge the queue was built for.
    #[must_use]
    pub const fn edge_id(&self) -> EdgeId {
        self.edge_id
    }

    /// Whether the queue was dropped: no sender and no receiver of it is left.
    #[must_use]
    pub fn is_dropped(&self) -> bool {
        self.shared.strong_count() == 0
    }
}

impl<P> Clone for QueueWatch<P> {
    fn clone(&self) -> Self {
        Self {
            edge_id: self.edge_id,
            shared: Weak::clone(&self.shared),
        }
    }
}

impl<P> fmt::Debug for QueueWatch<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueWatch")
            .field("edge_id", &self.edge_id)
            .field("dropped", &self.is_dropped())
            .finish()
    }
}

/// A weak reference to the queue of one `inference.graph.Exposure` in one run, as [`QueueWatch`]
/// is to the queue of an edge: it keeps neither side alive and tells whether the queue was
/// dropped, which it is once the node's end and the caller's end are both gone.
pub struct ExposureWatch<P> {
    exposure_id: ExposureId,
    shared: Weak<Shared<P>>,
}

impl<P> ExposureWatch<P> {
    /// The exposure the queue was built for.
    #[must_use]
    pub const fn exposure_id(&self) -> ExposureId {
        self.exposure_id
    }

    /// Whether the queue was dropped: no sender and no receiver of it is left.
    #[must_use]
    pub fn is_dropped(&self) -> bool {
        self.shared.strong_count() == 0
    }
}

impl<P> Clone for ExposureWatch<P> {
    fn clone(&self) -> Self {
        Self {
            exposure_id: self.exposure_id,
            shared: Weak::clone(&self.shared),
        }
    }
}

impl<P> fmt::Debug for ExposureWatch<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExposureWatch")
            .field("exposure_id", &self.exposure_id)
            .field("dropped", &self.is_dropped())
            .finish()
    }
}

/// The sending side of an [`EdgeQueue`]. Cloning it adds a sender; the receiver sees the end of
/// the edge once every sender is dropped.
pub struct QueueSender<P> {
    shared: Arc<Shared<P>>,
}

impl<P> QueueSender<P> {
    /// The number of envelopes queued now.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    /// Whether no envelope is queued now.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The number of envelopes discarded so far: those the overload policy discarded or replaced,
    /// and those still queued when the receiver was dropped.
    #[must_use]
    pub fn discarded(&self) -> u64 {
        self.shared.discarded()
    }
}

impl<P> Clone for QueueSender<P> {
    fn clone(&self) -> Self {
        self.shared.state().senders += 1;
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<P> Drop for QueueSender<P> {
    fn drop(&mut self) {
        let mut state = self.shared.state();
        state.senders -= 1;
        let receiver = if state.senders == 0 {
            state.waiting_receiver.take()
        } else {
            None
        };
        drop(state);
        wake(receiver);
    }
}

impl<P: Send> OutputEndpoint<P> for QueueSender<P> {
    fn send(&self, envelope: EventEnvelope<P>) -> EndpointFuture<'_, Result<(), SendError<P>>> {
        let mut slot = Some(envelope);
        Box::pin(future::poll_fn(move |cx| {
            self.shared.poll_send(cx, &mut slot)
        }))
    }
}

/// The receiving side of an [`EdgeQueue`]. Dropping it closes the edge: the envelopes still
/// queued are discarded and counted, and every later send returns [`SendError::Closed`].
pub struct QueueReceiver<P> {
    shared: Arc<Shared<P>>,
}

impl<P> QueueReceiver<P> {
    /// The number of envelopes queued now.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    /// Whether no envelope is queued now.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The number of envelopes discarded so far: those the overload policy discarded or replaced,
    /// and those still queued when the receiver was dropped.
    #[must_use]
    pub fn discarded(&self) -> u64 {
        self.shared.discarded()
    }
}

impl<P> Drop for QueueReceiver<P> {
    fn drop(&mut self) {
        let mut state = self.shared.state();
        state.receiver = false;
        state.discarded += state.items.len() as u64;
        state.items.clear();
        let senders = std::mem::take(&mut state.waiting_senders);
        drop(state);
        wake(senders);
    }
}

impl<P: Send> InputEndpoint<P> for QueueReceiver<P> {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<P>>> {
        Box::pin(future::poll_fn(move |cx| self.shared.poll_recv(cx)))
    }
}
