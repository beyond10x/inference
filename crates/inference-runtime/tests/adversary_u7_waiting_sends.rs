//! Adversary pass 1 on `story:waiting-sends-land-in-the-order-they-started-waiting`.
//!
//! Hand-driven cases on what the unit's own tests do not reach: a granted place whose send is
//! given up, a passed place granted again, a waiting send that changes its waker, a raise that
//! makes a placed send stale, and a close with landed, granted and waiting sends at once. Then a
//! seeded schedule model under every overload policy: with every waiting send polled after each
//! step, envelopes reach the receiver in the order their sends started; under every schedule the
//! queue holds at most its capacity, `known_contents` is truthful, every envelope is received,
//! refused with itself, withdrawn or counted as discarded exactly once, and no task that can make
//! progress is left unwoken.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use inference_core::endpoint::{
    ClosedToSenders, EndpointFuture, InputEndpoint, KnownContents, OutputEndpoint, SendError,
};
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Edge, Essentiality, InteractionMode, NodeRole, OverloadPolicy,
    PipelineSpec, PortTraits, modality,
};
use inference_runtime::{EdgeQueue, QueueReceiver, QueueSender};

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

fn queue(
    capacity: usize,
    overload: OverloadPolicy,
    epochs: HashMap<GenerationId, i64>,
) -> (QueueSender<u32>, QueueReceiver<u32>) {
    EdgeQueue::new(&edge(capacity, overload), epochs)
        .expect("a supported policy")
        .split()
}

fn item(
    producer: NodeId,
    stream: StreamId,
    generation: Option<GenerationId>,
    n: u32,
) -> EventEnvelope<u32> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: stream,
        sequence: i64::from(n),
        producer,
        port: "out".to_owned(),
        kind: "inference.stream.TextDelta".to_owned(),
        generation_id: generation,
        timestamp: i64::from(n),
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: n,
    }
}

#[derive(Default)]
struct Flag(AtomicBool);

impl Flag {
    fn set(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn clear(&self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::Release);
    }
}

fn flag() -> (Arc<Flag>, Waker) {
    let flag = Arc::new(Flag::default());
    let waker = Waker::from(Arc::clone(&flag));
    (flag, waker)
}

fn poll<F: Future + ?Sized>(future: Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(waker))
}

fn now<T>(future: impl Future<Output = T>) -> T {
    match poll(pin!(future), Waker::noop()) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the future did not complete at once"),
    }
}

fn take(receiver: &mut QueueReceiver<u32>) -> Option<u32> {
    now(receiver.recv()).map(|envelope| envelope.payload)
}

fn landed(sent: &Poll<Result<(), SendError<u32>>>) -> bool {
    matches!(sent, Poll::Ready(Ok(())))
}

/// A take grants the one free place to the older of two waiting sends; that send is dropped
/// before it polls. Its place goes to the younger send, which is woken and lands at its next
/// poll, and a send after it is not held: the queue never keeps room it does not grant.
#[test]
fn a_granted_place_given_up_before_its_send_polls_goes_to_the_next_waiting_send() {
    let (sender, mut receiver) = queue(1, OverloadPolicy::Pause, HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    now(sender.send(item(producer, stream, None, 1))).expect("room");
    let (_, older_waker) = flag();
    let (younger_woken, younger_waker) = flag();
    let mut older = sender.send(item(producer, stream, None, 2));
    let mut younger = sender.send(item(producer, stream, None, 3));
    assert!(poll(older.as_mut(), &older_waker).is_pending());
    assert!(poll(younger.as_mut(), &younger_waker).is_pending());
    assert_eq!(take(&mut receiver), Some(1));
    younger_woken.clear();
    drop(older);
    assert!(
        younger_woken.set(),
        "the place the dropped send held went to the waiting send, which must be woken to land"
    );
    assert_eq!(receiver.known_contents(), KnownContents::Empty);
    let sent = poll(younger.as_mut(), &younger_waker);
    assert!(
        landed(&sent),
        "the younger send holds the room the older gave up, and answered {sent:?}"
    );
    assert_eq!(take(&mut receiver), Some(3));
    now(sender.send(item(producer, stream, None, 4)))
        .expect("nothing waits and the queue is empty");
    assert_eq!(take(&mut receiver), Some(4));
    assert_eq!(receiver.discarded(), 0);
}

/// The receiver passes the place of a send that has not landed and takes a younger envelope.
/// The passed send keeps its age: it is granted room again before a send that arrives after
/// the pass, and the queue never holds more than its capacity.
#[test]
fn a_passed_send_is_granted_room_again_before_a_send_that_arrives_after_the_pass() {
    let (sender, mut receiver) = queue(2, OverloadPolicy::Pause, HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    now(sender.send(item(producer, stream, None, 1))).expect("room");
    now(sender.send(item(producer, stream, None, 2))).expect("room");
    let mut passed = sender.send(item(producer, stream, None, 3));
    let mut younger = sender.send(item(producer, stream, None, 4));
    assert!(poll(passed.as_mut(), Waker::noop()).is_pending());
    assert!(poll(younger.as_mut(), Waker::noop()).is_pending());
    assert_eq!(take(&mut receiver), Some(1));
    assert_eq!(take(&mut receiver), Some(2));
    assert!(landed(&poll(younger.as_mut(), Waker::noop())));
    assert_eq!(
        take(&mut receiver),
        Some(4),
        "the residual: the unlanded place is passed"
    );

    let mut arriving = sender.send(item(producer, stream, None, 5));
    let mut last = sender.send(item(producer, stream, None, 6));
    let arrived = poll(arriving.as_mut(), Waker::noop());
    let lasted = poll(last.as_mut(), Waker::noop());
    let passed_sent = poll(passed.as_mut(), Waker::noop());
    assert!(
        receiver.len() <= 2,
        "a queue of capacity 2 holds {} envelopes after the passed send landed",
        receiver.len()
    );
    let mut received = Vec::new();
    let mut polls = vec![arrived, lasted, passed_sent];
    for _ in 0..4 {
        if let Some(payload) = take_now(&mut receiver) {
            received.push(payload);
        }
        polls.push(poll(passed.as_mut(), Waker::noop()));
        polls.push(poll(arriving.as_mut(), Waker::noop()));
        polls.push(poll(last.as_mut(), Waker::noop()));
        assert!(receiver.len() <= 2);
    }
    assert_eq!(
        received,
        [3, 5, 6],
        "the send of 3 was passed before the sends of 5 and 6 started and kept its age; polls \
         answered {polls:?}"
    );
}

/// The payload the receiver hands over now, if any; never waits.
fn take_now(receiver: &mut QueueReceiver<u32>) -> Option<u32> {
    match poll(receiver.recv().as_mut(), Waker::noop()) {
        Poll::Ready(envelope) => envelope.map(|envelope| envelope.payload),
        Poll::Pending => None,
    }
}

/// A waiting send polled again by a task with another waker, as a future moved between tasks
/// is: room the queue makes later must wake the waker of its latest poll.
#[test]
fn a_waiting_send_polled_again_with_another_waker_is_woken_through_the_new_one() {
    let (sender, mut receiver) = queue(1, OverloadPolicy::Pause, HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    now(sender.send(item(producer, stream, None, 1))).expect("room");
    let (_, first_waker) = flag();
    let (second, second_waker) = flag();
    let mut waiting = sender.send(item(producer, stream, None, 2));
    assert!(poll(waiting.as_mut(), &first_waker).is_pending());
    assert!(poll(waiting.as_mut(), &second_waker).is_pending());
    assert_eq!(take(&mut receiver), Some(1));
    assert!(
        second.set(),
        "the take granted the waiting send a place and must wake the task now polling it"
    );
    assert!(landed(&poll(waiting.as_mut(), &second_waker)));
    assert_eq!(take(&mut receiver), Some(2));
}

/// Under `drop_stale_epochs` a raise makes a send that holds a place stale: its place goes to
/// the next waiting send, which is woken; the stale send completes as discarded; the raising
/// send waits behind the one granted before it.
#[test]
fn a_raise_that_makes_a_placed_send_stale_gives_its_place_to_the_next_waiting_send() {
    let (old, new) = (GenerationId::generate(), GenerationId::generate());
    let epochs = HashMap::from([(old, 1), (new, 2)]);
    let (sender, mut receiver) = queue(1, OverloadPolicy::DropStaleEpochs, epochs);
    let (raising, other, third) = (NodeId::generate(), NodeId::generate(), NodeId::generate());
    now(sender.send(item(other, StreamId::generate(), None, 1))).expect("room");
    let (stale_woken, stale_waker) = flag();
    let (next_woken, next_waker) = flag();
    let mut stale = sender.send(item(raising, StreamId::generate(), Some(old), 2));
    let mut next = sender.send(item(third, StreamId::generate(), None, 3));
    assert!(poll(stale.as_mut(), &stale_waker).is_pending());
    assert!(poll(next.as_mut(), &next_waker).is_pending());
    assert_eq!(
        take(&mut receiver),
        Some(1),
        "the take grants the send of 2 a place"
    );
    stale_woken.clear();
    next_woken.clear();
    let mut raise = sender.send(item(raising, StreamId::generate(), Some(new), 4));
    assert!(
        poll(raise.as_mut(), Waker::noop()).is_pending(),
        "the place the raise took from the stale send goes to the send of 3, older than the raise"
    );
    assert!(stale_woken.set() && next_woken.set());
    assert_eq!(receiver.known_contents(), KnownContents::Empty);
    assert!(landed(&poll(next.as_mut(), &next_waker)));
    assert!(landed(&poll(stale.as_mut(), &stale_waker)));
    assert_eq!(receiver.discarded(), 1, "the stale send is discarded, once");
    assert_eq!(take(&mut receiver), Some(3));
    assert!(landed(&poll(raise.as_mut(), Waker::noop())));
    assert_eq!(take(&mut receiver), Some(4));
    assert_eq!(receiver.discarded(), 1);
}

/// A close with a landed envelope, a landed send behind an unlanded place, an unlanded place
/// and two waiting sends: the receiver hands over what landed and ends, every other send is
/// woken and refused with its own envelope, and nothing is counted as discarded.
#[test]
fn a_close_with_landed_granted_and_waiting_sends_returns_every_unlanded_envelope() {
    let (sender, mut receiver) = queue(3, OverloadPolicy::Pause, HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    for n in 1..=3 {
        now(sender.send(item(producer, stream, None, n))).expect("room");
    }
    let wakers: Vec<_> = (0..4).map(|_| flag()).collect();
    let mut sends: Vec<_> = (4..=7)
        .map(|n| sender.send(item(producer, stream, None, n)))
        .collect();
    for (send, (_, waker)) in sends.iter_mut().zip(&wakers) {
        assert!(poll(send.as_mut(), waker).is_pending());
    }
    assert_eq!(take(&mut receiver), Some(1));
    assert_eq!(take(&mut receiver), Some(2));
    assert!(landed(&poll(sends[1].as_mut(), &wakers[1].1)));
    for (woken, _) in &wakers {
        woken.clear();
    }
    assert_eq!(receiver.close_to_senders(), ClosedToSenders::Holding);
    assert_eq!(take(&mut receiver), Some(3));
    assert_eq!(take(&mut receiver), Some(5));
    assert_eq!(take(&mut receiver), None);
    for at in [0, 2, 3] {
        assert!(wakers[at].0.set(), "the close woke the send of {}", at + 4);
        match poll(sends[at].as_mut(), &wakers[at].1) {
            Poll::Ready(Err(SendError::Closed(envelope))) => {
                assert_eq!(envelope.payload, u32::try_from(at).expect("small") + 4);
            }
            other => panic!("the send of {} answered {other:?}", at + 4),
        }
    }
    assert_eq!(receiver.discarded(), 0);
}

/// A small fixed-seed generator: the same seed walks the same schedule on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).expect("small")).expect("small")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Schedule {
    /// Every waiting send is polled after each step, in a random order.
    Prompt,
    /// A send or the receiver is polled only once its waker ran, and every woken send is
    /// polled after each step: an executor that runs woken tasks before the receiver's next
    /// step. The order then rests on the queue's wakes alone.
    WokenPrompt,
    /// A send or the receiver is polled only once its waker ran.
    WokenOnly,
}

#[derive(Debug, Clone, Copy)]
enum End {
    Drain,
    Close,
    DropReceiver,
}

struct Waiting<'a> {
    n: u32,
    woken: Arc<Flag>,
    waker: Waker,
    future: EndpointFuture<'a, Result<(), SendError<u32>>>,
}

const PRODUCERS: usize = 3;
const EPOCHS: i64 = 6;
const STEPS: usize = 80;

#[derive(Default)]
struct Ledger {
    started: Vec<u32>,
    received: Vec<u32>,
    refused: Vec<u32>,
    withdrawn: Vec<u32>,
}

/// Polls `waiting`; answers whether the send ended, and records how.
fn poll_waiting(waiting: &mut Waiting<'_>, ledger: &mut Ledger, ctx: &str) -> bool {
    waiting.woken.clear();
    match poll(waiting.future.as_mut(), &waiting.waker) {
        Poll::Pending => false,
        Poll::Ready(Ok(())) => true,
        Poll::Ready(Err(SendError::Closed(envelope) | SendError::Full(envelope))) => {
            assert_eq!(
                envelope.payload, waiting.n,
                "{ctx}: a refused send carries its own envelope"
            );
            ledger.refused.push(waiting.n);
            true
        }
        Poll::Ready(Err(other)) => panic!("{ctx}: the queue answered {other:?}"),
    }
}

struct Receiving {
    woken: Arc<Flag>,
    waker: Waker,
    waiting: bool,
}

/// One receive under the truthfulness rule: `NotEmpty` hands an envelope over at once, `Empty`
/// waits. Under `WokenOnly` a waiting receiver whose waker did not run must read `Empty`.
fn receive(
    receiver: &mut QueueReceiver<u32>,
    receiving: &mut Receiving,
    ledger: &mut Ledger,
    schedule: Schedule,
    ctx: &str,
) -> bool {
    let known = receiver.known_contents();
    if schedule != Schedule::Prompt && receiving.waiting && !receiving.woken.set() {
        assert_eq!(
            known,
            KnownContents::Empty,
            "{ctx}: an envelope landed and the waiting receiver was not woken"
        );
        return false;
    }
    receiving.woken.clear();
    match poll(receiver.recv().as_mut(), &receiving.waker) {
        Poll::Ready(Some(envelope)) => {
            assert_eq!(
                known,
                KnownContents::NotEmpty,
                "{ctx}: Empty, then handed over"
            );
            receiving.waiting = false;
            ledger.received.push(envelope.payload);
            true
        }
        Poll::Ready(None) => panic!("{ctx}: the open queue with a sender ended"),
        Poll::Pending => {
            assert_eq!(
                known,
                KnownContents::Empty,
                "{ctx}: NotEmpty, and recv waited"
            );
            receiving.waiting = true;
            false
        }
    }
}

fn is_subsequence(part: &[u32], whole: &[u32]) -> bool {
    let mut whole = whole.iter();
    part.iter().all(|x| whole.any(|y| y == x))
}

/// What the schedules reached, so a sweep that never made a send wait cannot pass for one
/// that did.
#[derive(Debug, Default)]
struct Reach {
    waited: usize,
    given_up_waiting: usize,
    out_of_start_order: usize,
}

#[allow(clippy::too_many_lines)]
fn model(
    policy: OverloadPolicy,
    capacity: usize,
    schedule: Schedule,
    end: End,
    seed: u64,
    reach: &mut Reach,
) {
    let ctx = format!("{policy:?} capacity {capacity} {schedule:?} {end:?} seed {seed}");
    let producers: Vec<NodeId> = (0..PRODUCERS).map(|_| NodeId::generate()).collect();
    let streams: Vec<StreamId> = (0..PRODUCERS).map(|_| StreamId::generate()).collect();
    let generations: Vec<Vec<GenerationId>> = (0..PRODUCERS)
        .map(|_| (0..EPOCHS).map(|_| GenerationId::generate()).collect())
        .collect();
    let mut epochs = HashMap::new();
    for per in &generations {
        for (epoch, generation) in per.iter().enumerate() {
            epochs.insert(*generation, i64::try_from(epoch).expect("small"));
        }
    }
    let (sender, mut receiver) = queue(capacity, policy, epochs);
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut current = [1_usize; PRODUCERS];
    let mut ledger = Ledger::default();
    let mut waiting: Vec<Waiting<'_>> = Vec::new();
    let (rx_woken, rx_waker) = flag();
    let mut receiving = Receiving {
        woken: rx_woken,
        waker: rx_waker,
        waiting: false,
    };
    let mut next = 0_u32;

    let poll_round =
        |waiting: &mut Vec<Waiting<'_>>, ledger: &mut Ledger, rng: &mut Rng, woken_only: bool| {
            let mut order: Vec<usize> = (0..waiting.len()).collect();
            for at in (1..order.len()).rev() {
                order.swap(at, rng.below(at + 1));
            }
            let mut ended = Vec::new();
            for at in order {
                if woken_only && !waiting[at].woken.set() {
                    continue;
                }
                if poll_waiting(&mut waiting[at], ledger, &ctx) {
                    ended.push(at);
                }
            }
            ended.sort_unstable();
            let progressed = !ended.is_empty();
            for at in ended.into_iter().rev() {
                waiting.remove(at);
            }
            progressed
        };

    for _ in 0..STEPS {
        match rng.below(10) {
            0..=3 => {
                let p = rng.below(PRODUCERS);
                let epoch = match rng.below(12) {
                    0 => {
                        current[p] = (current[p] + 1).min(generations[p].len() - 1);
                        current[p]
                    }
                    1 => current[p].saturating_sub(1),
                    _ => current[p],
                };
                next += 1;
                ledger.started.push(next);
                let (woken, waker) = flag();
                let mut sent = Waiting {
                    n: next,
                    woken,
                    waker,
                    future: sender.send(item(
                        producers[p],
                        streams[p],
                        Some(generations[p][epoch]),
                        next,
                    )),
                };
                if !poll_waiting(&mut sent, &mut ledger, &ctx) {
                    reach.waited += 1;
                    waiting.push(sent);
                }
            }
            4 | 5 => {
                let _ = poll_round(
                    &mut waiting,
                    &mut ledger,
                    &mut rng,
                    schedule != Schedule::Prompt,
                );
            }
            6 | 7 => {
                let _ = receive(&mut receiver, &mut receiving, &mut ledger, schedule, &ctx);
            }
            _ => {
                if !waiting.is_empty() {
                    let at = rng.below(waiting.len());
                    let given_up = waiting.remove(at);
                    reach.given_up_waiting += 1;
                    ledger.withdrawn.push(given_up.n);
                }
            }
        }
        if schedule == Schedule::Prompt {
            let _ = poll_round(&mut waiting, &mut ledger, &mut rng, false);
        }
        if schedule == Schedule::WokenPrompt {
            let mut rounds = 0;
            while waiting.iter().any(|w| w.woken.set()) {
                rounds += 1;
                assert!(rounds < 100, "{ctx}: wakes without progress, a waker storm");
                let _ = poll_round(&mut waiting, &mut ledger, &mut rng, true);
            }
        }
        assert!(
            receiver.len() <= capacity,
            "{ctx}: {} envelopes in a queue of capacity {capacity}",
            receiver.len()
        );
        assert_eq!(sender.len(), receiver.len());
    }

    match end {
        End::Drain => {
            let mut idle = 0;
            let mut rounds = 0_u32;
            while !waiting.is_empty() || !receiver.is_empty() {
                rounds += 1;
                assert!(
                    rounds < 10_000,
                    "{ctx}: wakes without progress, a waker storm"
                );
                let mut progressed = poll_round(
                    &mut waiting,
                    &mut ledger,
                    &mut rng,
                    schedule != Schedule::Prompt,
                );
                progressed |= receive(&mut receiver, &mut receiving, &mut ledger, schedule, &ctx);
                progressed |= waiting.iter().any(|w| w.woken.set()) || receiving.woken.set();
                if progressed {
                    idle = 0;
                    continue;
                }
                idle += 1;
                assert!(
                    idle < 3,
                    "{ctx}: nothing woken, nothing landed, sends still wait"
                );
                let waiting_now = waiting.len();
                let woke = poll_round(&mut waiting, &mut ledger, &mut rng, false);
                assert!(
                    !woke,
                    "{ctx}: a send completed when polled though its waker never ran"
                );
                assert!(
                    waiting_now == 0 || !receiver.is_empty(),
                    "{ctx}: {waiting_now} sends wait on a queue holding {} envelopes and \
                     nothing to grant: room is held that no send can use",
                    receiver.len()
                );
            }
        }
        End::Close => {
            let held = receiver.len();
            for w in &waiting {
                w.woken.clear();
            }
            let answer = receiver.close_to_senders();
            assert_eq!(
                answer,
                if held == 0 {
                    ClosedToSenders::Empty
                } else {
                    ClosedToSenders::Holding
                },
                "{ctx}"
            );
            for _ in 0..held {
                let envelope = now(receiver.recv()).expect("what landed is handed over");
                ledger.received.push(envelope.payload);
            }
            assert!(
                now(receiver.recv()).is_none(),
                "{ctx}: the closed queue ends"
            );
            for mut w in waiting.drain(..) {
                assert!(w.woken.set(), "{ctx}: the close woke the send of {}", w.n);
                let before = ledger.refused.len();
                assert!(poll_waiting(&mut w, &mut ledger, &ctx));
                assert_eq!(
                    ledger.refused.len(),
                    before + 1,
                    "{ctx}: refused after a close"
                );
            }
        }
        End::DropReceiver => {
            let held = u64::try_from(receiver.len()).expect("small");
            let before = receiver.discarded();
            for w in &waiting {
                w.woken.clear();
            }
            drop(receiver);
            assert_eq!(sender.discarded(), before + held, "{ctx}");
            for mut w in waiting.drain(..) {
                assert!(w.woken.set(), "{ctx}: the drop woke the send of {}", w.n);
                let refused = ledger.refused.len();
                assert!(poll_waiting(&mut w, &mut ledger, &ctx));
                assert_eq!(
                    ledger.refused.len(),
                    refused + 1,
                    "{ctx}: refused after the drop"
                );
            }
        }
    }

    let mut seen = ledger.received.clone();
    seen.extend(&ledger.refused);
    seen.extend(&ledger.withdrawn);
    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        seen.len(),
        "{ctx}: an envelope accounted twice"
    );
    let lost = ledger.started.len() - seen.len();
    assert_eq!(
        sender.discarded(),
        u64::try_from(lost).expect("small"),
        "{ctx}: every envelope neither received, refused nor withdrawn is counted as discarded"
    );
    if matches!(policy, OverloadPolicy::Pause | OverloadPolicy::Fail)
        && !matches!(end, End::DropReceiver)
    {
        assert_eq!(lost, 0, "{ctx}: {policy:?} discards nothing");
    }
    if schedule != Schedule::WokenOnly && !matches!(policy, OverloadPolicy::Coalesce) {
        assert!(
            is_subsequence(&ledger.received, &ledger.started),
            "{ctx}: every waiting send, or every woken one, was polled after each step, so \
             envelopes must arrive in \
             the order their sends started: started {:?}, received {:?}",
            ledger.started,
            ledger.received
        );
    }
    if !is_subsequence(&ledger.received, &ledger.started) {
        reach.out_of_start_order += 1;
    }
}

fn sweep(policy: OverloadPolicy) -> Reach {
    let mut reach = Reach::default();
    for capacity in 1..=3 {
        for schedule in [Schedule::Prompt, Schedule::WokenPrompt, Schedule::WokenOnly] {
            for end in [End::Drain, End::Close, End::DropReceiver] {
                for seed in 1..=150 {
                    model(policy, capacity, schedule, end, seed, &mut reach);
                }
            }
        }
    }
    reach
}

/// The sweep made sends wait, gave some up while they waited, and, under `WokenOnly`, reached
/// the decided residual, where a passed send lands after younger ones: the order checks above
/// ran against schedules that exercise grants and passes, not only an idle queue.
fn assert_reached(reach: &Reach) {
    assert!(
        reach.waited > 1000 && reach.given_up_waiting > 100 && reach.out_of_start_order > 0,
        "the schedules reached too little: {reach:?}"
    );
}

#[test]
fn seeded_schedules_under_pause_keep_order_capacity_truth_and_wakeups() {
    assert_reached(&sweep(OverloadPolicy::Pause));
}

#[test]
fn seeded_schedules_under_drop_stale_epochs_keep_order_capacity_truth_and_wakeups() {
    assert_reached(&sweep(OverloadPolicy::DropStaleEpochs));
}

#[test]
fn seeded_schedules_under_fail_drop_and_coalesce_keep_capacity_truth_and_accounting() {
    for policy in [
        OverloadPolicy::Fail,
        OverloadPolicy::Drop,
        OverloadPolicy::Coalesce,
    ] {
        let _ = sweep(policy);
    }
}
