//! Second adversary pass on a run's ingress around a send its caller gives up and around the
//! drain (`story:run-ingress-admitted-send-outlives-the-drain`): what the caller can tell after a
//! give-up, the sequence a withdrawn send gives back, and sends racing a drain from another
//! thread under every policy the ingress can wait or refuse under.
//!
//! The single-threaded cases poll futures by hand; the threaded case runs each round in a thread
//! of its own and fails once [`PATIENCE`] passed.

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
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
use inference_runtime::run::RunState;
use inference_testkit::graph::PipelinePayload;
use inference_testkit::llm::{MockConfiguration, MockGenerator};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

const PATIENCE: Duration = Duration::from_secs(30);

const STOP: &str = "stop";

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

/// Runs `case` in a thread of its own and fails once it took longer than [`PATIENCE`].
fn bounded<T: Send + 'static>(case: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _test_waits_or_gave_up = done.send(case());
    });
    finished
        .recv_timeout(PATIENCE)
        .expect("the case finished within its time bound")
}

#[derive(Default)]
struct Released(Mutex<(bool, Option<Waker>)>);

impl Released {
    fn release(&self) {
        let waker = {
            let mut state = lock(&self.0);
            state.0 = true;
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

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

/// A node that hands the test the receiving end of its input when it starts and drains once the
/// test released it.
struct Hand {
    input: mpsc::Sender<Input>,
    released: Arc<Released>,
    outputs: Vec<NodeOutput<PipelinePayload>>,
}

impl Hand {
    fn boxed(
        input: &mpsc::Sender<Input>,
        released: &Arc<Released>,
    ) -> Box<dyn NodeImplementation<PipelinePayload>> {
        Box::new(Self {
            input: input.clone(),
            released: Arc::clone(released),
            outputs: Vec::new(),
        })
    }
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
    sequence: i64,
}

impl Caller {
    fn new(run_id: RunId) -> Self {
        Self {
            run_id,
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    fn envelope(&self, kind: &str, payload: PipelinePayload) -> EventEnvelope<PipelinePayload> {
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: "user text".to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }

    fn text_in(&mut self, content: &str) -> EventEnvelope<PipelinePayload> {
        self.sequence += 1;
        let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: content.to_owned(),
        });
        self.envelope(event.name(), PipelinePayload::Stream(event))
    }

    fn stop(&self) -> EventEnvelope<PipelinePayload> {
        self.envelope(
            ControlKind::Stop.as_str(),
            PipelinePayload::Control(ControlKind::Stop),
        )
    }
}

fn label(envelope: &EventEnvelope<PipelinePayload>) -> String {
    match &envelope.payload {
        PipelinePayload::Stream(StreamEvent::TranscriptCommitted(committed)) => {
            committed.text.clone()
        }
        PipelinePayload::Control(ControlKind::Stop) => STOP.to_owned(),
        other => format!("{other:?}"),
    }
}

fn take(input: &mut Input) -> String {
    let envelope = block_on(input.recv()).expect("the edge holds an envelope or one arrives");
    label(&envelope)
}

/// What the node's input hands over now; panics if it would wait, so a single-threaded case
/// fails rather than hangs.
fn take_now(input: &mut Input) -> String {
    match poll(input.recv().as_mut(), Waker::noop()) {
        Poll::Ready(Some(envelope)) => label(&envelope),
        Poll::Ready(None) => panic!("the node's input ended"),
        Poll::Pending => panic!("the node's input holds nothing to take now"),
    }
}

fn outcome(sent: &Poll<Result<(), SendError<PipelinePayload>>>) -> String {
    match sent {
        Poll::Pending => "Pending".to_owned(),
        Poll::Ready(Ok(())) => "Ok".to_owned(),
        Poll::Ready(Err(SendError::Closed(_))) => "Closed".to_owned(),
        Poll::Ready(Err(SendError::SequenceNotIncreasing { last, .. })) => {
            format!("SequenceNotIncreasing(last {last})")
        }
        Poll::Ready(Err(other)) => format!("{other:?}"),
    }
}

/// A run of [`flat`] made Active, with the ingress, the node's input and the egress end.
struct Active {
    controller: RunController<PipelinePayload>,
    ingress: Ingress,
    input: Input,
    reply: Input,
    released: Arc<Released>,
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
        |_: &Node| Hand::boxed(&inputs, &released),
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
        reply,
        released,
        run_id,
    }
}

/// What a caller of the run's ingress can tell after it gave a send up: only a send of the same
/// envelope again tells. A send waits for room with its sequence under way until the queue takes
/// the envelope or the send settles. When the queue took the envelope, the edge keeps its
/// sequence and the same envelope again is refused as `SequenceNotIncreasing`, which the run
/// records as a refusal; when the send was given up first, its sequence was given back and the
/// same envelope again is taken. Either way the node receives each envelope once.
#[test]
fn an_ingress_send_given_up_is_told_delivered_or_not_only_by_sending_it_again() {
    let Active {
        controller,
        ingress,
        mut input,
        reply: _reply,
        released: _released,
        run_id,
    } = active(OverloadPolicy::Pause);
    let mut caller = Caller::new(run_id);
    let noop = Waker::noop();

    assert!(matches!(
        poll(ingress.send(caller.text_in("first")).as_mut(), noop),
        Poll::Ready(Ok(()))
    ));
    let second = caller.text_in("second");
    let mut received = Vec::new();
    {
        let mut waiting = ingress.send(second.clone());
        assert!(poll(waiting.as_mut(), noop).is_pending());
        received.push(take_now(&mut input));
        if let Poll::Ready(Some(envelope)) = poll(input.recv().as_mut(), noop) {
            received.push(label(&envelope));
        }
    }
    let after_delivered = poll(ingress.send(second).as_mut(), noop);
    if matches!(after_delivered, Poll::Ready(Ok(()))) {
        // The given-up send waited for room and never had its envelope taken, so its sequence
        // was given back and the same envelope sent again was taken; the node takes it before
        // the next send.
        received.push(take_now(&mut input));
    }

    assert!(matches!(
        poll(ingress.send(caller.text_in("third")).as_mut(), noop),
        Poll::Ready(Ok(()))
    ));
    let fourth = caller.text_in("fourth");
    {
        let mut waiting = ingress.send(fourth.clone());
        assert!(poll(waiting.as_mut(), noop).is_pending());
        received.push(take_now(&mut input));
    }
    let after_withdrawn = poll(ingress.send(fourth).as_mut(), noop);
    if let Poll::Ready(Some(envelope)) = poll(input.recv().as_mut(), noop) {
        received.push(label(&envelope));
    }

    assert_eq!(
        received,
        ["first", "second", "third", "fourth"],
        "each envelope reached the node once"
    );
    assert!(
        matches!(after_delivered, Poll::Ready(Ok(())))
            && matches!(after_withdrawn, Poll::Ready(Ok(()))),
        "story:run-ingress-admitted-send-outlives-the-drain: a send given up before it landed \
         was never delivered, so both sent again are taken; the first answered {} and the \
         second {}",
        outcome(&after_delivered),
        outcome(&after_withdrawn)
    );
    let refusals = controller.refusals();
    assert_eq!(
        refusals
            .iter()
            .map(|refusal| refusal.sequence)
            .collect::<Vec<_>>(),
        Vec::<i64>::new(),
        "story:run-ingress-admitted-send-outlives-the-drain: no envelope sent again was refused"
    );
}

/// Two sends on one stream wait on the ingress at once and the caller gives both up, the older
/// first. Neither envelope was taken, so the edge must give both sequences back and the older
/// envelope sent again must be taken.
#[test]
fn two_waiting_ingress_sends_given_up_oldest_first_leave_the_older_sendable_again() {
    let Active {
        controller: _controller,
        ingress,
        mut input,
        reply: _reply,
        released: _released,
        run_id,
    } = active(OverloadPolicy::Pause);
    let mut caller = Caller::new(run_id);
    let noop = Waker::noop();

    assert!(matches!(
        poll(ingress.send(caller.text_in("first")).as_mut(), noop),
        Poll::Ready(Ok(()))
    ));
    let second = caller.text_in("second");
    let third = caller.text_in("third");
    let mut older = ingress.send(second.clone());
    let mut younger = ingress.send(third);
    assert!(poll(older.as_mut(), noop).is_pending());
    assert!(poll(younger.as_mut(), noop).is_pending());
    drop(older);
    drop(younger);
    assert_eq!(take_now(&mut input), "first");
    let again = poll(ingress.send(second).as_mut(), noop);
    assert!(
        matches!(again, Poll::Ready(Ok(()))),
        "neither given-up envelope was taken, yet the older one sent again answered {}",
        outcome(&again)
    );
    assert_eq!(take_now(&mut input), "second");
}

/// What one round of [`round`] saw.
struct Round {
    answers: Vec<(String, String)>,
    received: Vec<String>,
    answered: bool,
    state: RunState,
    refusals: usize,
}

/// One round: a caller thread sends forty envelopes and then `stop` on the ingress, retrying a
/// full queue under `fail` with the envelope it got back; a reader thread takes the node's input
/// up to `stop`; the test asks for the drain from its own thread once the caller had `at` sends
/// answered. Returns what the caller was answered, what the node received, and the run's end.
fn round(policy: OverloadPolicy, at: usize) -> Round {
    let Active {
        mut controller,
        ingress,
        mut input,
        reply,
        released,
        run_id,
    } = active(policy);
    let requests = controller.requests();
    let drive = thread::spawn(move || {
        let answered = block_on(controller.drive()).is_ok();
        (
            answered,
            controller.run().state(),
            controller.refusals().len(),
        )
    });
    let reader = thread::spawn(move || {
        let mut received = Vec::new();
        while received.last().map(String::as_str) != Some(STOP) {
            received.push(take(&mut input));
        }
        released.release();
        received
    });
    let (answered_tx, answered_rx) = mpsc::channel();
    let sender = thread::spawn(move || {
        let mut caller = Caller::new(run_id);
        let mut answers = Vec::new();
        for n in 1..=40 {
            let mut envelope = caller.text_in(&n.to_string());
            let answer = loop {
                match block_on(ingress.send(envelope)) {
                    Err(SendError::Full(back)) => {
                        envelope = back;
                        thread::yield_now();
                    }
                    Ok(()) => break ("Ok".to_owned(), n.to_string()),
                    Err(SendError::Closed(back)) => break ("Closed".to_owned(), label(&back)),
                    Err(other) => break (format!("{other:?}"), n.to_string()),
                }
            };
            answers.push(answer);
            let _test_waits_or_gone = answered_tx.send(n);
        }
        let mut stop = caller.stop();
        loop {
            match block_on(ingress.send(stop)) {
                Err(SendError::Full(back)) => {
                    stop = back;
                    thread::yield_now();
                }
                Ok(()) => break,
                Err(other) => panic!("the ingress refused stop: {other:?}"),
            }
        }
        (answers, ingress)
    });
    while let Ok(n) = answered_rx.recv_timeout(PATIENCE) {
        if n >= at {
            break;
        }
    }
    requests.drain();
    let (answers, _ingress) = sender.join().expect("the caller does not panic");
    let received = reader.join().expect("the reader does not panic");
    let (answered, state, refusals) = drive.join().expect("the run loop does not panic");
    drop(reply);
    Round {
        answers,
        received,
        answered,
        state,
        refusals,
    }
}

/// A drain asked for from another thread while the caller sends on the ingress, under `pause`,
/// `fail` and `drop_stale_epochs`: every send answered `Ok` reached the node, in order; every
/// other send answered `Closed` with its own envelope, which the node never received; once one
/// send was refused every later one is; the run completes, no consumer counted as gone and no
/// refusal recorded.
#[test]
fn ingress_sends_racing_a_drain_from_another_thread_are_each_delivered_or_refused_closed() {
    for policy in [
        OverloadPolicy::Pause,
        OverloadPolicy::Fail,
        OverloadPolicy::DropStaleEpochs,
    ] {
        for at in [1, 3, 7, 13, 20, 29, 38] {
            let Round {
                answers,
                received,
                answered,
                state,
                refusals,
            } = bounded(move || round(policy, at));
            let delivered: Vec<&str> = received
                .iter()
                .map(String::as_str)
                .filter(|label| *label != STOP)
                .collect();
            let ok: Vec<&str> = answers
                .iter()
                .filter(|(answer, _)| answer == "Ok")
                .map(|(_, label)| label.as_str())
                .collect();
            assert_eq!(
                delivered, ok,
                "{policy:?} at {at}: the node received what the caller was answered Ok for"
            );
            let first_refused = answers.iter().position(|(answer, _)| answer != "Ok");
            eprintln!(
                "{policy:?} at {at}: {} answered Ok, the first refusal at {first_refused:?}",
                ok.len()
            );
            if let Some(first) = first_refused {
                assert!(
                    answers[first..]
                        .iter()
                        .all(|(answer, _)| answer == "Closed"),
                    "{policy:?} at {at}: after a refusal every send is refused as closed: \
                     {answers:?}"
                );
                assert!(
                    answers[first..]
                        .iter()
                        .enumerate()
                        .all(|(offset, (_, label))| *label == (first + offset + 1).to_string()),
                    "{policy:?} at {at}: each refused send got its own envelope back"
                );
            }
            assert!(
                answered && state == RunState::Completed && refusals == 0,
                "{policy:?} at {at}: the run answered {answered}, ended {state:?} with \
                 {refusals} refusals"
            );
        }
    }
}
