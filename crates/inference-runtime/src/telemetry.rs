//! Telemetry: the separate best-effort channel of `docs/design/inference.md` line 688, on which
//! metrics and traces travel so that they cannot block realtime inference.
//!
//! A [`TelemetrySink`] is offered one [`Telemetry`] record per envelope on a run's event path, by
//! the [`Recorder`](crate::record::Recorder) it is attached to
//! ([`Recorder::with_telemetry`](crate::record::Recorder::with_telemetry)). Offering never waits:
//! a sink that cannot take a record now drops it and counts the drop. [`channel`] makes the sink
//! this crate provides, a bounded, lossy queue: its [`TelemetrySender`] takes a record while the
//! queue has room and its [`TelemetryReceiver`] is held, and drops it otherwise, so a slow consumer
//! or one that never reads loses telemetry and delays no node. Both ends read the counts of what
//! was taken and what was dropped.

use std::collections::VecDeque;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};

use inference_core::id::{NodeId, RunId, StreamId};

/// One telemetry record: an envelope a run's event path carried, as the recorder took it.
/// `merge_order` is the order the recorder assigned it; `node_id` the node that emitted it, none
/// for what the caller sent on an ingress; `kind`, `stream_id` and `sequence` are the envelope's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Telemetry {
    pub merge_order: u64,
    pub run_id: RunId,
    pub node_id: Option<NodeId>,
    pub kind: String,
    pub stream_id: StreamId,
    pub sequence: i64,
}

/// Where telemetry is offered. An implementation must not wait: the recorder offers on the path
/// every node sends on, so a sink that waits delays the node. A sink that cannot take a record now
/// drops it.
pub trait TelemetrySink: Send + Sync {
    /// Takes `telemetry`, or drops it, at once.
    fn offer(&self, telemetry: Telemetry);
}

/// What the two ends of one channel share.
struct Shared {
    capacity: usize,
    queue: Mutex<VecDeque<Telemetry>>,
    receiver_gone: AtomicBool,
    accepted: AtomicU64,
    dropped: AtomicU64,
}

/// The sending end of a telemetry channel, the [`TelemetrySink`] it is offered through. Clones
/// send on the same channel.
#[derive(Clone)]
pub struct TelemetrySender {
    shared: Arc<Shared>,
}

/// The receiving end of a telemetry channel. Once it is dropped, every record offered is dropped.
pub struct TelemetryReceiver {
    shared: Arc<Shared>,
}

/// A bounded, lossy telemetry channel holding at most `capacity` records: a record offered while
/// it is full, while its queue is locked by the receiver, or once the receiver is gone, is dropped
/// and counted, and the offer returns at once.
#[must_use]
pub fn channel(capacity: NonZeroUsize) -> (TelemetrySender, TelemetryReceiver) {
    let shared = Arc::new(Shared {
        capacity: capacity.get(),
        queue: Mutex::new(VecDeque::with_capacity(capacity.get())),
        receiver_gone: AtomicBool::new(false),
        accepted: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
    });
    (
        TelemetrySender {
            shared: Arc::clone(&shared),
        },
        TelemetryReceiver { shared },
    )
}

impl Shared {
    fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::Acquire)
    }

    fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Acquire)
    }

    fn drop_one(&self) {
        self.dropped.fetch_add(1, Ordering::AcqRel);
    }
}

impl TelemetrySender {
    /// How many records the channel has dropped.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.shared.dropped()
    }

    /// How many records the channel has taken.
    #[must_use]
    pub fn accepted(&self) -> u64 {
        self.shared.accepted()
    }
}

impl TelemetrySink for TelemetrySender {
    fn offer(&self, telemetry: Telemetry) {
        if self.shared.receiver_gone.load(Ordering::Acquire) {
            self.shared.drop_one();
            return;
        }
        let mut queue = match self.shared.queue.try_lock() {
            Ok(queue) => queue,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.shared.drop_one();
                return;
            }
        };
        if queue.len() < self.shared.capacity {
            queue.push_back(telemetry);
            self.shared.accepted.fetch_add(1, Ordering::AcqRel);
        } else {
            self.shared.drop_one();
        }
    }
}

impl TelemetryReceiver {
    /// The oldest record the channel holds, if it holds one; never waits for one.
    pub fn try_recv(&mut self) -> Option<Telemetry> {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    /// How many records the channel has dropped.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.shared.dropped()
    }

    /// How many records the channel has taken.
    #[must_use]
    pub fn accepted(&self) -> u64 {
        self.shared.accepted()
    }
}

impl Drop for TelemetryReceiver {
    fn drop(&mut self) {
        self.shared.receiver_gone.store(true, Ordering::Release);
    }
}

impl fmt::Debug for TelemetrySender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelemetrySender")
            .field("accepted", &self.accepted())
            .field("dropped", &self.dropped())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for TelemetryReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelemetryReceiver")
            .field("accepted", &self.accepted())
            .field("dropped", &self.dropped())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use inference_core::id::{RunId, StreamId};

    use super::{Telemetry, TelemetrySink, channel};

    fn record(sequence: i64) -> Telemetry {
        Telemetry {
            merge_order: u64::try_from(sequence).expect("positive"),
            run_id: RunId::generate(),
            node_id: None,
            kind: "inference.stream.TextDelta".to_owned(),
            stream_id: StreamId::generate(),
            sequence,
        }
    }

    #[test]
    fn a_full_channel_drops_and_counts_what_it_has_no_room_for() {
        let (sender, mut receiver) = channel(NonZeroUsize::new(2).expect("2"));
        for sequence in 1..=5 {
            sender.offer(record(sequence));
        }
        assert_eq!((sender.accepted(), sender.dropped()), (2, 3));
        let kept: Vec<i64> = std::iter::from_fn(|| receiver.try_recv())
            .map(|telemetry| telemetry.sequence)
            .collect();
        assert_eq!(kept, [1, 2], "the oldest records are kept");
        sender.offer(record(6));
        assert_eq!(
            receiver.try_recv().map(|telemetry| telemetry.sequence),
            Some(6)
        );
        assert_eq!((receiver.accepted(), receiver.dropped()), (3, 3));
    }

    #[test]
    fn once_the_receiver_is_gone_every_record_is_dropped() {
        let (sender, receiver) = channel(NonZeroUsize::new(4).expect("4"));
        drop(receiver);
        sender.offer(record(1));
        assert_eq!((sender.accepted(), sender.dropped()), (0, 1));
    }
}
