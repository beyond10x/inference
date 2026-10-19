//! Adversary pass 1 on the review fixes: every envelope a queue removes without delivering it is
//! counted (receiver drop, coalesce eviction, epoch purge, stale arrival), coalesce keeps each
//! arrival and the order within each stream, and a raised epoch completes the stale pending sends
//! of its producer while a current-epoch pending send is still woken by the next take. Futures
//! are polled with counting wakers, and the threaded cases run under a deadline, so a lost wake
//! fails a case instead of hanging the suite.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Edge, Essentiality, InteractionMode, NodeRole, OverloadPolicy,
    PipelineSpec, PortTraits, modality,
};
use inference_runtime::{EdgeQueue, QueueReceiver, QueueSender};

const DEADLINE: Duration = Duration::from_secs(60);

/// The one profile the case declares, and no adapter: what `Declaration::build` reads.
struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

fn edge(capacity: usize, overload: OverloadPolicy) -> Edge {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let traits = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let spec = PipelineSpec {
        name: "edge".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::Continuous,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let capacity = NonZeroUsize::new(capacity).expect("a positive capacity");
    let mut declaration = Declaration::new();
    let profile_id = profile.profile_id();
    let catalog = Profiles(profile);
    declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let out = pipeline.output::<modality::Text>(&source, "out", traits);
            let into = pipeline.input::<modality::Text>(&sink, "in", traits);
            pipeline.connect(&out, &into, capacity, overload);
        })
        .expect("the profile is declared");
    declaration.edges().next().expect("one edge").clone()
}

type Payload = (u32, u32);

fn queue(
    capacity: usize,
    overload: OverloadPolicy,
    epochs: HashMap<GenerationId, i64>,
) -> (QueueSender<Payload>, QueueReceiver<Payload>) {
    EdgeQueue::new(&edge(capacity, overload), epochs)
        .expect("a supported policy")
        .split()
}

fn item(
    producer: NodeId,
    stream: StreamId,
    generation: Option<GenerationId>,
    payload: Payload,
) -> EventEnvelope<Payload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: stream,
        sequence: i64::from(payload.1),
        producer,
        port: "out".to_owned(),
        kind: "inference.stream.TextDelta".to_owned(),
        generation_id: generation,
        timestamp: i64::from(payload.1),
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

#[derive(Default)]
struct Count(AtomicUsize);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Task {
    count: Arc<Count>,
    waker: Waker,
}

impl Task {
    fn new() -> Self {
        let count = Arc::new(Count::default());
        let waker = Waker::from(Arc::clone(&count));
        Self { count, waker }
    }

    fn poll<F: Future + ?Sized>(&self, future: Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(&self.waker))
    }

    fn woken(&self) -> usize {
        self.count.0.load(Ordering::SeqCst)
    }
}

fn now<T>(future: impl Future<Output = T>) -> T {
    match Task::new().poll(pin!(future)) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the future did not complete at once"),
    }
}

/// A fixed-seed generator, so a failing sequence is the same on every run.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        u32::try_from((self.0 >> 33) % u64::from(below)).expect("below a u32")
    }
}

#[test]
fn a_raised_epoch_completes_every_stale_pending_send_and_keeps_the_current_one_waiting_for_a_take()
{
    let (old, new) = (GenerationId::generate(), GenerationId::generate());
    let (sender, mut receiver) = queue(
        2,
        OverloadPolicy::DropStaleEpochs,
        HashMap::from([(old, 1), (new, 2)]),
    );
    let (a, b, c) = (NodeId::generate(), NodeId::generate(), NodeId::generate());
    let stream = StreamId::generate();
    now(sender.send(item(b, stream, None, (0, 1)))).expect("room for it");
    now(sender.send(item(b, stream, None, (0, 2)))).expect("room for it");
    let (first, second, current) = (Task::new(), Task::new(), Task::new());
    let mut stale_one = sender.send(item(a, stream, Some(old), (1, 1)));
    let mut stale_two = sender.send(item(a, stream, Some(old), (1, 2)));
    let mut waiting = sender.send(item(c, stream, Some(old), (2, 1)));
    assert!(first.poll(stale_one.as_mut()).is_pending());
    assert!(second.poll(stale_two.as_mut()).is_pending());
    assert!(current.poll(waiting.as_mut()).is_pending());
    let raiser = Task::new();
    let mut raising = sender.send(item(a, stream, Some(new), (1, 3)));
    assert!(
        raiser.poll(raising.as_mut()).is_pending(),
        "still full of b"
    );
    assert!(
        first.woken() > 0 && second.woken() > 0,
        "a stale send was not woken"
    );
    assert!(matches!(
        first.poll(stale_one.as_mut()),
        Poll::Ready(Ok(()))
    ));
    assert!(matches!(
        second.poll(stale_two.as_mut()),
        Poll::Ready(Ok(()))
    ));
    assert_eq!((receiver.len(), receiver.discarded()), (2, 2));
    let woken_before = current.woken();
    assert!(
        current.poll(waiting.as_mut()).is_pending(),
        "c's epoch is current and the queue is still full"
    );
    let raiser_before = raiser.woken();
    let taken = now(receiver.recv()).expect("b's first item");
    assert_eq!(taken.payload, (0, 1));
    assert!(
        current.woken() > woken_before,
        "the take did not wake c's current-epoch send after the raise re-registered it"
    );
    assert!(
        raiser.woken() > raiser_before,
        "the take did not wake the raiser"
    );
    assert!(matches!(
        current.poll(waiting.as_mut()),
        Poll::Ready(Ok(()))
    ));
    assert!(raiser.poll(raising.as_mut()).is_pending(), "full again");
    let taken = now(receiver.recv()).expect("b's second item");
    assert_eq!(taken.payload, (0, 2));
    assert!(matches!(raiser.poll(raising.as_mut()), Poll::Ready(Ok(()))));
    drop((stale_one, stale_two, waiting, raising));
    drop(sender);
    let mut rest = Vec::new();
    while let Some(envelope) = now(receiver.recv()) {
        rest.push(envelope.payload);
    }
    assert_eq!(rest, vec![(2, 1), (1, 3)]);
    assert_eq!(receiver.discarded(), 2);
}

#[test]
fn a_purge_that_frees_room_lets_a_woken_current_epoch_send_in_without_a_take() {
    let (old, new) = (GenerationId::generate(), GenerationId::generate());
    let (sender, receiver) = queue(
        3,
        OverloadPolicy::DropStaleEpochs,
        HashMap::from([(old, 1), (new, 2)]),
    );
    let (a, c) = (NodeId::generate(), NodeId::generate());
    let stream = StreamId::generate();
    for n in 1..=3 {
        now(sender.send(item(a, stream, Some(old), (1, n)))).expect("room for it");
    }
    let (stale, current) = (Task::new(), Task::new());
    let mut stale_send = sender.send(item(a, stream, Some(old), (1, 4)));
    let mut current_send = sender.send(item(c, stream, None, (2, 1)));
    assert!(stale.poll(stale_send.as_mut()).is_pending());
    assert!(current.poll(current_send.as_mut()).is_pending());
    now(sender.send(item(a, stream, Some(new), (1, 5)))).expect("the purge frees room");
    assert_eq!((receiver.len(), receiver.discarded()), (1, 3));
    assert!(
        stale.woken() > 0 && current.woken() > 0,
        "a waiter was not woken"
    );
    assert!(matches!(
        stale.poll(stale_send.as_mut()),
        Poll::Ready(Ok(()))
    ));
    assert!(matches!(
        current.poll(current_send.as_mut()),
        Poll::Ready(Ok(()))
    ));
    assert_eq!((receiver.len(), receiver.discarded()), (2, 4));
}

#[test]
fn coalesce_matches_the_decision_on_a_random_sequence_and_accounts_for_every_send() {
    for capacity in 1..=4 {
        let (sender, mut receiver) = queue(capacity, OverloadPolicy::Coalesce, HashMap::new());
        let producer = NodeId::generate();
        let streams: Vec<StreamId> = (0..3).map(|_| StreamId::generate()).collect();
        let mut random = Lcg(0x5eed_0000 + u64::try_from(capacity).expect("small"));
        let mut sent = [0_u32; 3];
        let mut model: std::collections::VecDeque<Payload> = std::collections::VecDeque::new();
        let mut model_discarded = 0_u64;
        let mut delivered: Vec<Payload> = Vec::new();
        for step in 0..3000 {
            if random.next(4) == 0 {
                if let Some(expected) = model.pop_front() {
                    let envelope = now(receiver.recv()).expect("queued");
                    assert_eq!(
                        envelope.payload, expected,
                        "capacity {capacity}, step {step}"
                    );
                    delivered.push(envelope.payload);
                }
                continue;
            }
            let s = random.next(3);
            let index = usize::try_from(s).expect("small");
            sent[index] += 1;
            let payload = (s, sent[index]);
            now(sender.send(item(producer, streams[index], None, payload)))
                .expect("coalesce never refuses");
            if model.len() < capacity {
                model.push_back(payload);
            } else {
                if let Some(newest) = model.iter_mut().rev().find(|queued| queued.0 == s) {
                    *newest = payload;
                } else {
                    model.pop_front();
                    model.push_back(payload);
                }
                model_discarded += 1;
            }
            assert_eq!(
                (receiver.len(), receiver.discarded()),
                (model.len(), model_discarded),
                "capacity {capacity}, step {step}"
            );
        }
        drop(sender);
        while let Some(envelope) = now(receiver.recv()) {
            assert_eq!(
                Some(envelope.payload),
                model.pop_front(),
                "capacity {capacity}"
            );
            delivered.push(envelope.payload);
        }
        let total: u32 = sent.iter().sum();
        assert_eq!(
            u64::try_from(delivered.len()).expect("small") + receiver.discarded(),
            u64::from(total),
            "capacity {capacity}: delivered plus discarded is every send"
        );
        for s in 0..3 {
            let each: Vec<u32> = delivered
                .iter()
                .filter(|(stream, _)| *stream == s)
                .map(|(_, n)| *n)
                .collect();
            assert!(
                each.windows(2).all(|pair| pair[0] < pair[1]),
                "capacity {capacity}: stream {s} out of order: {each:?}"
            );
        }
    }
}

#[test]
fn dropping_the_receiver_counts_what_it_clears_under_every_policy() {
    let generation = GenerationId::generate();
    for overload in [
        OverloadPolicy::Pause,
        OverloadPolicy::Fail,
        OverloadPolicy::Drop,
        OverloadPolicy::Coalesce,
        OverloadPolicy::DropStaleEpochs,
    ] {
        let (sender, mut receiver) = queue(3, overload, HashMap::from([(generation, 1)]));
        let (producer, stream) = (NodeId::generate(), StreamId::generate());
        for n in 1..=3 {
            now(sender.send(item(producer, stream, Some(generation), (0, n)))).expect("room");
        }
        assert_eq!(now(receiver.recv()).expect("queued").payload, (0, 1));
        now(sender.send(item(producer, stream, Some(generation), (0, 4)))).expect("room");
        let waiter = Task::new();
        let mut late = sender.send(item(producer, stream, Some(generation), (0, 5)));
        let polled = waiter.poll(late.as_mut());
        let accepted = u64::from(matches!(polled, Poll::Ready(Ok(()))));
        let before = sender.discarded();
        let queued = u64::try_from(sender.len()).expect("small");
        assert_eq!(queued, 3, "{overload:?}: full when the receiver goes");
        drop(receiver);
        assert_eq!(
            sender.discarded(),
            before + queued,
            "{overload:?}: the receiver's drop did not count the {queued} it cleared"
        );
        assert_eq!(
            1 + sender.discarded(),
            4 + accepted,
            "{overload:?}: delivered plus discarded is every accepted send"
        );
        if polled.is_pending() {
            assert!(
                waiter.woken() > 0,
                "{overload:?}: the closed edge woke nobody"
            );
            match waiter.poll(late.as_mut()) {
                Poll::Ready(Err(SendError::Closed(envelope))) => {
                    assert_eq!(envelope.payload, (0, 5));
                }
                other => panic!("{overload:?}: expected the closed error, got {other:?}"),
            }
        }
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

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

const NODES: u32 = 3;
const EPOCHS: u32 = 4;
const PER_EPOCH: u32 = 150;

/// Each of `NODES` producing nodes runs one thread per epoch, all at once, so a node's older
/// epochs keep sending while its newer ones arrive: their pending sends go stale under a slow
/// consumer. Every thread must finish, every accepted send must be delivered or counted, and a
/// node's deliveries never go back to an older epoch.
#[test]
fn drop_stale_epochs_with_racing_epochs_of_one_node_finishes_and_accounts_for_every_send() {
    let generations: Vec<GenerationId> = (0..EPOCHS).map(|_| GenerationId::generate()).collect();
    let lookup: HashMap<GenerationId, i64> = generations
        .iter()
        .zip(1..)
        .map(|(generation, epoch)| (*generation, epoch))
        .collect();
    let epoch_of: HashMap<GenerationId, u32> = generations.iter().copied().zip(0..).collect();
    let (sender, receiver) = queue(2, OverloadPolicy::DropStaleEpochs, lookup);
    let (done, outcome) = mpsc::channel();
    thread::spawn(move || {
        let nodes: Vec<NodeId> = (0..NODES).map(|_| NodeId::generate()).collect();
        let mut producers = Vec::new();
        for (p, node) in (0..NODES).zip(nodes.iter().copied()) {
            for (e, generation) in (0..EPOCHS).zip(generations.iter().copied()) {
                let sender = sender.clone();
                producers.push(thread::spawn(move || {
                    let stream = StreamId::generate();
                    for n in 1..=PER_EPOCH {
                        let mut envelope = item(node, stream, Some(generation), (p, e * 1000 + n));
                        envelope.sequence = i64::from(n);
                        block_on(sender.send(envelope)).expect("never closed here");
                        if n % 13 == 0 {
                            thread::yield_now();
                        }
                    }
                }));
            }
        }
        drop(sender);
        let consumer = thread::spawn(move || {
            let mut receiver = receiver;
            let mut delivered = Vec::new();
            while let Some(envelope) = block_on(receiver.recv()) {
                let generation = envelope.generation_id.expect("every item has one");
                delivered.push((
                    envelope.payload.0,
                    epoch_of[&generation],
                    envelope.payload.1,
                ));
                if delivered.len() % 5 == 0 {
                    thread::sleep(Duration::from_micros(50));
                }
            }
            (delivered, receiver.discarded())
        });
        for producer in producers {
            producer.join().expect("producer");
        }
        let _ = done.send(consumer.join().expect("consumer"));
    });
    let (delivered, discarded) = outcome
        .recv_timeout(DEADLINE)
        .expect("a thread stayed parked: a wakeup was lost");
    assert_eq!(
        u64::try_from(delivered.len()).expect("small") + discarded,
        u64::from(NODES * EPOCHS * PER_EPOCH),
        "delivered plus discarded is every send"
    );
    for p in 0..NODES {
        let epochs: Vec<u32> = delivered
            .iter()
            .filter(|(node, _, _)| *node == p)
            .map(|(_, epoch, _)| *epoch)
            .collect();
        assert!(
            epochs.windows(2).all(|pair| pair[0] <= pair[1]),
            "node {p} went back to an older epoch: {epochs:?}"
        );
        assert!(
            epochs.last() == Some(&(EPOCHS - 1)),
            "node {p}: the newest epoch never arrived"
        );
    }
}
