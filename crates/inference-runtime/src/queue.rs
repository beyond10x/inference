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
//! - `pause`: the send waits until the consumer takes an envelope; nothing is lost. Waiting sends
//!   are served in the order they started waiting: room goes to the oldest as a place in queue
//!   order, a new send takes room only when none waits, and a send lands in its place by its own
//!   poll. The receiver sees only landed envelopes (`len` and `known_contents` count no place),
//!   passes a place whose send has not landed, and never waits on a caller; that send then waits
//!   again with its age and lands after the envelopes that landed before it.
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
//! the receiver yields what is queued and then `None`. The receiver can also close the queue to
//! its senders while it stays ([`InputEndpoint::close_to_senders`]): every send is then refused
//! as closed with its envelope, one waiting for room at its next poll, and the receiver yields
//! what is queued and then `None` without waiting. Waiting is a [`Future`](std::future::Future)
//! woken through its [`Waker`], so the queue runs under any executor.

use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context, Poll, Waker};

use inference_core::endpoint::{
    ClosedToSenders, EndpointFuture, InputEndpoint, KnownContents, OutputEndpoint, SendError,
};
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
        if !state.receiver || state.closed_to_senders || state.items.len() < shared.capacity.get() {
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

/// One place of the queue, in queue order: an envelope that landed, or the room granted to the
/// waiting send holding a ticket, whose envelope takes that place when the send's own poll lands
/// it. The receiver sees only landed envelopes.
enum Place<P> {
    Landed(Queued<P>),
    Granted(u64),
}

/// A send that found no room, in line: its age is its number, and its envelope stays with the
/// send.
struct Ticket {
    id: u64,
    producer: NodeId,
    epoch: Option<i64>,
    waker: Waker,
    placed: bool,
}

struct State<P> {
    /// The queue's places, oldest first; at most `capacity` of them.
    items: VecDeque<Place<P>>,
    /// The sends waiting for room, oldest first. Room goes to the oldest that is not stale and
    /// holds no place, and a new send takes room only when none such waits.
    tickets: VecDeque<Ticket>,
    next_ticket: u64,
    discarded: u64,
    newest_epochs: HashMap<NodeId, i64>,
    senders: usize,
    receiver: bool,
    /// The receiver closed the queue to its senders ([`InputEndpoint::close_to_senders`]).
    closed_to_senders: bool,
    /// Whether the receiver closes the queue to its senders when asked; not for the queue of an
    /// edge between two nodes of a run ([`QueueSender::keep_open_to_senders`]).
    closes: bool,
    waiting_senders: Vec<Waker>,
    waiting_receiver: Option<Waker>,
}

impl<P> State<P> {
    /// How many envelopes landed and are queued, the places no envelope landed in aside.
    fn landed(&self) -> usize {
        self.items
            .iter()
            .filter(|place| matches!(place, Place::Landed(_)))
            .count()
    }

    /// Whether `ticket` is of an epoch lower than its producer's newest.
    fn stale(&self, ticket: &Ticket) -> bool {
        ticket.epoch.is_some_and(|epoch| {
            self.newest_epochs
                .get(&ticket.producer)
                .is_some_and(|newest| epoch < *newest)
        })
    }

    /// Whether a send waits that is not stale and holds no place.
    fn waiting_without_place(&self) -> bool {
        self.tickets
            .iter()
            .any(|ticket| !ticket.placed && !self.stale(ticket))
    }

    /// Gives the room the queue has to the oldest tickets that are not stale and hold no place,
    /// each as a place at the back.
    fn grant(&mut self, capacity: usize) {
        for at in 0..self.tickets.len() {
            if self.items.len() >= capacity {
                break;
            }
            let eligible = self
                .tickets
                .get(at)
                .is_some_and(|ticket| !ticket.placed && !self.stale(ticket));
            if eligible && let Some(ticket) = self.tickets.get_mut(at) {
                ticket.placed = true;
                let id = ticket.id;
                self.items.push_back(Place::Granted(id));
            }
        }
    }

    fn ticket_at(&self, id: u64) -> Option<usize> {
        self.tickets.iter().position(|ticket| ticket.id == id)
    }

    /// Removes the place granted to the ticket `id`, if it holds one; the ticket keeps its age.
    fn remove_place(&mut self, id: u64) {
        if let Some(at) = self
            .items
            .iter()
            .position(|place| matches!(place, Place::Granted(held) if *held == id))
        {
            self.items.remove(at);
        }
        if let Some(at) = self.ticket_at(id)
            && let Some(ticket) = self.tickets.get_mut(at)
        {
            ticket.placed = false;
        }
    }

    /// Removes the places of tickets a raised epoch made stale.
    fn remove_stale_places(&mut self) {
        let stale: Vec<u64> = self
            .tickets
            .iter()
            .filter(|ticket| ticket.placed && self.stale(ticket))
            .map(|ticket| ticket.id)
            .collect();
        for id in stale {
            self.remove_place(id);
        }
    }

    /// Every task that waits for room: the waiting sends and the holders of a [`Room`].
    fn waiting_wakers(&mut self) -> Vec<Waker> {
        let mut wakers = std::mem::take(&mut self.waiting_senders);
        wakers.extend(self.tickets.iter().map(|ticket| ticket.waker.clone()));
        wakers
    }

    /// Removes every place no envelope landed in and every ticket: nothing that has not landed is
    /// kept or counted. Answers the wakers of the waiting tasks, whose sends are refused as
    /// closed at their next poll.
    fn close(&mut self) -> Vec<Waker> {
        let wakers = self.waiting_wakers();
        self.items.retain(|place| matches!(place, Place::Landed(_)));
        self.tickets.clear();
        wakers
    }
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
        self.state().landed()
    }

    fn discarded(&self) -> u64 {
        self.state().discarded
    }

    /// One attempt to send the envelope in `slot`, as a send that holds no ticket. A send that
    /// must wait takes a ticket, sets `ticket`, leaves the envelope in `slot` and registers the
    /// task; every other outcome takes the envelope.
    fn poll_send(
        &self,
        cx: &Context<'_>,
        slot: &mut Option<EventEnvelope<P>>,
        ticket: &mut Option<u64>,
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
        let capacity = self.capacity.get();
        let mut state = self.state();
        if !state.receiver || state.closed_to_senders {
            return Poll::Ready(Err(SendError::Closed(envelope)));
        }
        let mut woken = Vec::new();
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
                state.items.retain(|place| match place {
                    Place::Landed(queued) => {
                        queued.envelope.producer != producer
                            || queued.epoch.is_none_or(|queued| queued >= epoch)
                    }
                    Place::Granted(_) => true,
                });
                let stale = before - state.items.len();
                state.discarded += stale as u64;
                state.remove_stale_places();
                state.grant(capacity);
                // A raised epoch may have made a waiting send stale, or freed room for one: every
                // waiting send polls again, and a stale one completes as discarded.
                woken = state.waiting_wakers();
            }
        }
        if !state.waiting_without_place() && state.items.len() < capacity {
            state
                .items
                .push_back(Place::Landed(Queued { envelope, epoch }));
            let receiver = state.waiting_receiver.take();
            drop(state);
            wake(receiver);
            wake(woken);
            return Poll::Ready(Ok(()));
        }
        let outcome = match self.policy {
            Policy::Pause | Policy::DropStaleEpochs => {
                let id = state.next_ticket;
                state.next_ticket += 1;
                state.tickets.push_back(Ticket {
                    id,
                    producer: envelope.producer,
                    epoch,
                    waker: cx.waker().clone(),
                    placed: false,
                });
                *ticket = Some(id);
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
                if let Some(newest) = state.items.iter_mut().rev().find_map(|place| match place {
                    Place::Landed(queued) if queued.envelope.stream_id == stream_id => Some(queued),
                    Place::Landed(_) | Place::Granted(_) => None,
                }) {
                    *newest = arrival;
                } else {
                    state.items.pop_front();
                    state.items.push_back(Place::Landed(arrival));
                }
                state.discarded += 1;
                Poll::Ready(Ok(()))
            }
        };
        drop(state);
        wake(woken);
        outcome
    }

    /// One poll of the send holding `ticket`, whose envelope is in `slot`. Refused as closed with
    /// its envelope once the queue is closed; discarded when a raised epoch made it stale; landed
    /// in its place, which wakes the receiver, when it holds one; otherwise it waits. `None` when
    /// the queue no longer knows the ticket and is open: the send then starts over.
    fn poll_ticket(
        &self,
        cx: &Context<'_>,
        id: u64,
        slot: &mut Option<EventEnvelope<P>>,
        ticket: &mut Option<u64>,
    ) -> Option<Poll<Result<(), SendError<P>>>> {
        let capacity = self.capacity.get();
        let mut state = self.state();
        if !state.receiver || state.closed_to_senders {
            *ticket = None;
            let envelope = slot.take()?;
            return Some(Poll::Ready(Err(SendError::Closed(envelope))));
        }
        let at = state.ticket_at(id)?;
        let stale = state.tickets.get(at).is_some_and(|held| state.stale(held));
        let placed = state.tickets.get(at).is_some_and(|held| held.placed);
        if stale {
            *ticket = None;
            *slot = None;
            state.remove_place(id);
            state.tickets.remove(at);
            state.discarded += 1;
            state.grant(capacity);
            let woken = state.waiting_wakers();
            drop(state);
            wake(woken);
            return Some(Poll::Ready(Ok(())));
        }
        if !placed {
            if let Some(held) = state.tickets.get_mut(at)
                && !held.waker.will_wake(cx.waker())
            {
                held.waker = cx.waker().clone();
            }
            return Some(Poll::Pending);
        }
        *ticket = None;
        let held = state.tickets.remove(at)?;
        let Some(envelope) = slot.take() else {
            return Some(Poll::Ready(Ok(())));
        };
        let landed = Place::Landed(Queued {
            envelope,
            epoch: held.epoch,
        });
        match state
            .items
            .iter_mut()
            .find(|place| matches!(place, Place::Granted(granted) if *granted == id))
        {
            Some(place) => *place = landed,
            None => state.items.push_back(landed),
        }
        let receiver = state.waiting_receiver.take();
        drop(state);
        wake(receiver);
        Some(Poll::Ready(Ok(())))
    }

    /// The send holding `ticket` was dropped before it landed: its ticket leaves the line, and
    /// the place it held goes to the next waiting send.
    fn withdraw(&self, id: u64) {
        let capacity = self.capacity.get();
        let mut state = self.state();
        let Some(at) = state.ticket_at(id) else {
            return;
        };
        let placed = state.tickets.get(at).is_some_and(|held| held.placed);
        state.remove_place(id);
        state.tickets.remove(at);
        if !placed {
            return;
        }
        state.grant(capacity);
        let woken = state.waiting_wakers();
        drop(state);
        wake(woken);
    }

    /// One attempt to take the oldest landed envelope. Every place in front of it is passed over,
    /// its ticket keeps its age, and the room made goes to the oldest waiting sends; the receiver
    /// never waits on a place. A queue closed to its senders never waits: nothing more can land
    /// on it.
    fn poll_recv(&self, cx: &Context<'_>) -> Poll<Option<EventEnvelope<P>>> {
        let capacity = self.capacity.get();
        let mut state = self.state();
        if let Some(at) = state
            .items
            .iter()
            .position(|place| matches!(place, Place::Landed(_)))
        {
            let passed: Vec<Place<P>> = state.items.drain(..at).collect();
            for place in passed {
                if let Place::Granted(id) = place
                    && let Some(held) = state.ticket_at(id)
                    && let Some(ticket) = state.tickets.get_mut(held)
                {
                    ticket.placed = false;
                }
            }
            let taken = state.items.pop_front();
            state.grant(capacity);
            let woken = state.waiting_wakers();
            drop(state);
            wake(woken);
            return match taken {
                Some(Place::Landed(queued)) => Poll::Ready(Some(queued.envelope)),
                Some(Place::Granted(_)) | None => Poll::Pending,
            };
        }
        if state.senders == 0 || state.closed_to_senders {
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
            closed_to_senders: false,
            closes: true,
            tickets: VecDeque::new(),
            next_ticket: 0,
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

    /// Whether the receiver closed the queue to its senders
    /// ([`InputEndpoint::close_to_senders`]): a send it refuses as closed then was not delivered
    /// while the receiver stays, which reads what the queue held.
    pub(crate) fn is_closed_to_senders(&self) -> bool {
        self.shared.state().closed_to_senders
    }

    /// Makes the receiver answer [`ClosedToSenders::Unsupported`] when asked to close the queue
    /// to its senders, and close nothing: for the queue of an edge between two nodes of a run,
    /// called when the run wraps the queue's sender for its producer (the `drive` module).
    pub(crate) fn keep_open_to_senders(&self) {
        self.shared.state().closes = false;
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
        Box::pin(Sending {
            shared: &self.shared,
            slot: Some(envelope),
            ticket: None,
        })
    }
}

/// One send of a [`QueueSender`]: the envelope until the queue answers or the send waits, then
/// the ticket it holds. The envelope enters the queue only during a poll of this send that
/// answers `Ready(Ok)`; dropped while it waits, the send withdraws its ticket and passes on any
/// place it held.
struct Sending<'a, P> {
    shared: &'a Shared<P>,
    slot: Option<EventEnvelope<P>>,
    ticket: Option<u64>,
}

impl<P> Unpin for Sending<'_, P> {}

impl<P> Future for Sending<'_, P> {
    type Output = Result<(), SendError<P>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if let Some(id) = this.ticket
            && let Some(outcome) = this
                .shared
                .poll_ticket(cx, id, &mut this.slot, &mut this.ticket)
        {
            return outcome;
        }
        this.ticket = None;
        this.shared.poll_send(cx, &mut this.slot, &mut this.ticket)
    }
}

impl<P> Drop for Sending<'_, P> {
    fn drop(&mut self) {
        if let Some(id) = self.ticket.take() {
            self.shared.withdraw(id);
        }
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
        state.discarded += state.landed() as u64;
        state.items.clear();
        let senders = state.close();
        drop(state);
        wake(senders);
    }
}

impl<P: Send> InputEndpoint<P> for QueueReceiver<P> {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<P>>> {
        Box::pin(std::future::poll_fn(move |cx| self.shared.poll_recv(cx)))
    }

    /// Empty while nothing is queued, closed or not, and not empty otherwise; a `recv` future
    /// takes an envelope only when it completes, so dropping one before loses nothing.
    fn known_contents(&self) -> KnownContents {
        if self.is_empty() {
            KnownContents::Empty
        } else {
            KnownContents::NotEmpty
        }
    }

    /// In one step under the queue's lock: marks the queue closed to its senders, wakes every
    /// send waiting for room, so that its next poll is refused as closed with its envelope, and
    /// answers from what the queue holds. A send that pushed its envelope before the step was
    /// taken, and what it pushed is still handed over; one that had not is refused, so none
    /// answers `Ok` for an envelope the receiver never sees.
    ///
    /// The queue of an edge between two nodes of a run, which the run controller builds, is not
    /// closed: it answers [`ClosedToSenders::Unsupported`] and keeps every send as before, since a
    /// node that drains with its consumer, on a feedback cycle, may still send into it. Every
    /// other queue closes: a run's exposures, its ingress among them, and a queue built with
    /// [`EdgeQueue::new`] and split outside a run.
    fn close_to_senders(&mut self) -> ClosedToSenders {
        let mut state = self.shared.state();
        if !state.closes {
            return ClosedToSenders::Unsupported;
        }
        state.closed_to_senders = true;
        let answer = if state.landed() == 0 {
            ClosedToSenders::Empty
        } else {
            ClosedToSenders::Holding
        };
        let senders = state.close();
        drop(state);
        wake(senders);
        answer
    }
}
