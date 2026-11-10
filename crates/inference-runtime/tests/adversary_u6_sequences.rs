//! Adversary cases on the sequence bookkeeping of a run's ingress when sends end untaken
//! (`story:withdrawn-ingress-send-gives-back-its-sequence`): several sends under way on one
//! stream, given up or refused in every order, a duplicate sent while the first is still under
//! way, and a send that unwinds after the queue took its envelope.
//!
//! Every case polls futures by hand on one thread, with an ingress holding one envelope.

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeOutput, NodeRole, OverloadPolicy, PipelineSpec,
    PlannedPipeline, PortTraits, modality,
};
use inference_runtime::RunController;
use inference_testkit::graph::PipelinePayload;
use inference_testkit::llm::{MockConfiguration, MockGenerator};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

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
        thread::park_timeout(Duration::from_millis(50));
    }
}

fn poll<F: Future + ?Sized>(future: Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(waker))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct Released(Mutex<(bool, Option<Waker>)>);

impl Released {
    fn poll_released(&self, cx: &Context<'_>) -> Poll<()> {
        let mut state = lock(&self.0);
        if state.0 {
            return Poll::Ready(());
        }
        state.1 = Some(cx.waker().clone());
        Poll::Pending
    }
}

type Input = Box<dyn InputEndpoint<PipelinePayload>>;
type Ingress = Box<dyn OutputEndpoint<PipelinePayload>>;

/// A node that hands the test the receiving end of its input when it starts.
struct Hand {
    input: mpsc::Sender<Input>,
    released: Arc<Released>,
    outputs: Vec<NodeOutput<PipelinePayload>>,
}

impl NodeImplementation<PipelinePayload> for Hand {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        for input in endpoints.inputs {
            let _test_waits_or_gone = self.input.send(input.endpoint);
        }
        self.outputs = endpoints.outputs;
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let released = Arc::clone(&self.released);
        Box::pin(async move {
            future::poll_fn(|cx| released.poll_released(cx)).await;
            Ok(())
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.outputs.clear();
        Box::pin(future::ready(()))
    }
}

struct Fixture {
    profile: DeploymentProfile,
    adapter: Adapter,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        (self.adapter.adapter_id() == adapter_id).then_some(&self.adapter)
    }
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

/// One node between the ingress `user text`, holding one under `ingress`, and the egress
/// `reply`, holding one under `pause`.
fn flat(ingress: OverloadPolicy) -> (Arc<Declaration>, PlannedPipeline) {
    let generator = MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    );
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let adapter_id = generator.adapter().adapter_id();
    let catalog = Fixture {
        profile,
        adapter: generator.adapter().clone(),
    };
    let spec = PipelineSpec {
        name: "flat".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let mut declaration = Declaration::new();
    let (flat_id, ()) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let node = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input::<modality::Text>(&node, "prompt", TEXT);
            let answer = pipeline.output::<modality::Text>(&node, "answer", TEXT);
            pipeline.expose(&prompt, "user text", NonZeroUsize::MIN, ingress);
            pipeline.expose(&answer, "reply", NonZeroUsize::MIN, OverloadPolicy::Pause);
            pipeline.bind(&node, adapter_id).expect("known");
        })
        .expect("known");
    let validated = declaration.validate(flat_id).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");
    (Arc::new(declaration), planned)
}

struct Caller {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
}

impl Caller {
    fn new(run_id: RunId) -> Self {
        Self {
            run_id,
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
        }
    }

    /// User content `content` at `sequence` on the caller's stream.
    fn text_at(&self, content: &str, sequence: i64) -> EventEnvelope<PipelinePayload> {
        let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: content.to_owned(),
        });
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence,
            producer: self.producer,
            port: "user text".to_owned(),
            kind: event.name().to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: PipelinePayload::Stream(event),
        }
    }
}

fn label(envelope: &EventEnvelope<PipelinePayload>) -> String {
    match &envelope.payload {
        PipelinePayload::Stream(StreamEvent::TranscriptCommitted(committed)) => {
            committed.text.clone()
        }
        PipelinePayload::Control(ControlKind::Stop) => "stop".to_owned(),
        other => format!("{other:?}"),
    }
}

/// What the node's input hands over now, or none when it holds nothing.
fn take_now(input: &mut Input) -> Option<String> {
    match poll(input.recv().as_mut(), Waker::noop()) {
        Poll::Ready(Some(envelope)) => Some(label(&envelope)),
        Poll::Ready(None) => panic!("the node's input ended"),
        Poll::Pending => None,
    }
}

fn outcome(sent: &Poll<Result<(), SendError<PipelinePayload>>>) -> String {
    match sent {
        Poll::Pending => "Pending".to_owned(),
        Poll::Ready(Ok(())) => "Ok".to_owned(),
        Poll::Ready(Err(SendError::Closed(_))) => "Closed".to_owned(),
        Poll::Ready(Err(SendError::Full(_))) => "Full".to_owned(),
        Poll::Ready(Err(SendError::SequenceNotIncreasing { last, .. })) => {
            format!("Refused(last {last})")
        }
    }
}

/// One poll of a fresh send of `envelope`, the send dropped afterwards whatever it answered.
fn once(ingress: &Ingress, envelope: EventEnvelope<PipelinePayload>) -> String {
    outcome(&poll(ingress.send(envelope).as_mut(), Waker::noop()))
}

struct Active {
    controller: RunController<PipelinePayload>,
    ingress: Ingress,
    input: Input,
    _reply: Input,
    _released: Arc<Released>,
    run_id: RunId,
}

fn active(ingress: OverloadPolicy) -> Active {
    let (declaration, planned) = flat(ingress);
    let run_id = RunId::generate();
    let (inputs, handed) = mpsc::channel();
    let released = Arc::new(Released::default());
    let mut controller = RunController::create_as(
        run_id,
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| {
            Box::new(Hand {
                input: inputs.clone(),
                released: Arc::clone(&released),
                outputs: Vec::new(),
            }) as Box<dyn NodeImplementation<PipelinePayload>>
        },
    )
    .expect("planned");
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("prepares");
    let mut ends = controller.exposed();
    let ingress = ends.ingress("user text").expect("the ingress");
    let reply = ends.egress("reply").expect("the egress");
    block_on(controller.activate()).expect("activates");
    let input = handed.try_recv().expect("the node handed over its input");
    Active {
        controller,
        ingress,
        input,
        _reply: reply,
        _released: released,
        run_id,
    }
}

/// Every order of `items`, in lexicographic order of positions.
fn permutations(items: &[i64]) -> Vec<Vec<i64>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut all = Vec::new();
    for (at, &item) in items.iter().enumerate() {
        let mut rest = items.to_vec();
        rest.remove(at);
        for mut tail in permutations(&rest) {
            tail.insert(0, item);
            all.push(tail);
        }
    }
    all
}

/// Four sends on one stream wait at once behind a taken one, and the caller gives them up in
/// every one of the 24 orders. After each give-up a fresh envelope at the given-up sequence is
/// refused exactly when a send still under way, or the taken one, holds a sequence at least as
/// high, and the refusal names that highest sequence; otherwise it is admitted (and waits). After
/// all four are given up, the oldest is sent again and taken.
#[test]
fn sends_under_way_given_up_in_every_order_give_back_exactly_their_own_sequence() {
    let mut wrong = Vec::new();
    for order in permutations(&[2, 3, 4, 5]) {
        let Active {
            ingress,
            mut input,
            run_id,
            ..
        } = active(OverloadPolicy::Pause);
        let caller = Caller::new(run_id);
        let noop = Waker::noop();
        assert_eq!(once(&ingress, caller.text_at("1", 1)), "Ok");
        let mut waiting = Vec::new();
        for sequence in 2..=5 {
            let mut send = ingress.send(caller.text_at(&sequence.to_string(), sequence));
            assert!(poll(send.as_mut(), noop).is_pending());
            waiting.push((sequence, send));
        }
        for &given_up in &order {
            waiting.retain(|(sequence, _)| *sequence != given_up);
            let highest = waiting
                .iter()
                .map(|(sequence, _)| *sequence)
                .chain([1])
                .max()
                .expect("one at least");
            let expected = if given_up <= highest {
                format!("Refused(last {highest})")
            } else {
                "Pending".to_owned()
            };
            let probed = once(&ingress, caller.text_at("probe", given_up));
            if probed != expected {
                wrong.push(format!(
                    "order {order:?}: after giving up {given_up}, a fresh send at {given_up} \
                     answered {probed}, not {expected}"
                ));
            }
        }
        assert_eq!(take_now(&mut input).as_deref(), Some("1"));
        let again = once(&ingress, caller.text_at("2", 2));
        if again != "Ok" {
            wrong.push(format!(
                "order {order:?}: the oldest given-up envelope sent again answered {again}"
            ));
        }
        assert_eq!(take_now(&mut input).as_deref(), Some("2"));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// While a send of sequence 2 waits, the same envelope sent a second time is refused naming
/// sequence 2 as the stream's last: two sends of one sequence are never under way at once.
#[test]
fn a_duplicate_of_a_send_still_under_way_is_refused_naming_its_sequence() {
    let Active {
        controller,
        ingress,
        mut input,
        run_id,
        ..
    } = active(OverloadPolicy::Pause);
    let caller = Caller::new(run_id);
    let noop = Waker::noop();
    assert_eq!(once(&ingress, caller.text_at("first", 1)), "Ok");
    let second = caller.text_at("second", 2);
    let mut waiting = ingress.send(second.clone());
    assert!(poll(waiting.as_mut(), noop).is_pending());
    let mut duplicate = ingress.send(second);
    let answered = outcome(&poll(duplicate.as_mut(), noop));
    assert_eq!(
        answered, "Refused(last 2)",
        "a second send of the sequence under way must be refused"
    );
    drop(duplicate);
    assert_eq!(take_now(&mut input).as_deref(), Some("first"));
    assert!(matches!(poll(waiting.as_mut(), noop), Poll::Ready(Ok(()))));
    drop(waiting);
    assert_eq!(take_now(&mut input).as_deref(), Some("second"));
    assert_eq!(
        take_now(&mut input),
        None,
        "the node received `second` once"
    );
    let refusals = controller
        .refusals()
        .iter()
        .map(|refusal| (refusal.last, refusal.sequence))
        .collect::<Vec<_>>();
    assert_eq!(refusals, [(2, 2)]);
}

/// Two sends wait at once and room frees up. Whichever the queue takes first, once both are
/// taken the stream's last is the higher of the two: the same envelope at sequence 3 sent again
/// is refused naming 3, and the node received each envelope once.
#[test]
fn two_waiting_sends_taken_in_either_order_leave_the_higher_sequence_last() {
    let Active {
        ingress,
        mut input,
        run_id,
        ..
    } = active(OverloadPolicy::Pause);
    let caller = Caller::new(run_id);
    let noop = Waker::noop();
    assert_eq!(once(&ingress, caller.text_at("first", 1)), "Ok");
    let mut older = ingress.send(caller.text_at("second", 2));
    let mut younger = ingress.send(caller.text_at("third", 3));
    assert!(poll(older.as_mut(), noop).is_pending());
    assert!(poll(younger.as_mut(), noop).is_pending());
    let mut received = vec![take_now(&mut input).expect("first is queued")];
    let mut ends = [None, None];
    for _ in 0..4 {
        if ends[1].is_none()
            && let Poll::Ready(sent) = poll(younger.as_mut(), noop)
        {
            ends[1] = Some(sent.is_ok());
        }
        if ends[0].is_none()
            && let Poll::Ready(sent) = poll(older.as_mut(), noop)
        {
            ends[0] = Some(sent.is_ok());
        }
        if let Some(label) = take_now(&mut input) {
            received.push(label);
        }
    }
    assert_eq!(ends, [Some(true), Some(true)], "both sends were taken");
    drop(older);
    drop(younger);
    received.sort();
    assert_eq!(received, ["first", "second", "third"]);
    let again = once(&ingress, caller.text_at("third", 3));
    assert_eq!(
        again, "Refused(last 3)",
        "sequence 3 was taken, so the stream's last cannot have fallen below it"
    );
    assert_eq!(take_now(&mut input), None, "nothing was sent twice");
}

/// Under `fail` a send to a full ingress is refused as full, which takes nothing: its sequence
/// is given back and the same envelope sent once there is room is taken.
#[test]
fn a_send_refused_as_full_gives_back_its_sequence() {
    let Active {
        ingress,
        mut input,
        run_id,
        ..
    } = active(OverloadPolicy::Fail);
    let caller = Caller::new(run_id);
    assert_eq!(once(&ingress, caller.text_at("first", 1)), "Ok");
    let second = caller.text_at("second", 2);
    assert_eq!(once(&ingress, second.clone()), "Full");
    assert_eq!(take_now(&mut input).as_deref(), Some("first"));
    assert_eq!(once(&ingress, second), "Ok");
    assert_eq!(take_now(&mut input).as_deref(), Some("second"));
}

/// A waker that panics when it is woken.
struct Panics;

impl Wake for Panics {
    fn wake(self: Arc<Self>) {
        panic!("the waker panics");
    }
}

/// The queue takes the envelope and the send then unwinds: waking the node's input, which waits
/// with a waker that panics, panics inside the send's poll after the push. The queue holds the
/// envelope, so its sequence is the stream's: the same envelope sent again must be refused, and
/// the node must receive it once. A send that gives its sequence back whenever it is dropped
/// unanswered gives it back here too, and the envelope is delivered twice.
#[test]
fn a_send_that_unwinds_after_the_queue_took_its_envelope_keeps_its_sequence() {
    let Active {
        ingress,
        mut input,
        run_id,
        ..
    } = active(OverloadPolicy::Pause);
    let caller = Caller::new(run_id);
    let panics = Waker::from(Arc::new(Panics));
    assert!(
        poll(input.recv().as_mut(), &panics).is_pending(),
        "the node's input waits with the panicking waker"
    );
    let first = caller.text_at("first", 1);
    let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut send = ingress.send(first.clone());
        poll(send.as_mut(), Waker::noop()).is_ready()
    }));
    assert!(unwound.is_err(), "the send unwound out of its poll");
    assert_eq!(
        take_now(&mut input).as_deref(),
        Some("first"),
        "the queue took the envelope before the send unwound"
    );
    let again = once(&ingress, first);
    let twice = take_now(&mut input);
    assert!(
        again.starts_with("Refused") && twice.is_none(),
        "the queue took `first` at sequence 1, yet the same envelope sent again answered \
         {again} and the node then received {twice:?}"
    );
}
