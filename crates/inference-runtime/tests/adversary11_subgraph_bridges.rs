//! Adversary cases against the subgraph node's bridges and its child run's lifecycle
//! (`story:subgraph-execution`, `story:pipeline-exposed-ports`).
//!
//! - a parent cancelled while the caller does not read the parent's egress fed by the subgraph
//!   node must still end: the egress bridge's send must give way to the cancel;
//! - a child whose ingress declares the overload policy `fail` must not stall its own bridges:
//!   a full ingress must not keep the child's thread from forwarding the child's egress;
//! - a subgraph node whose output port feeds two parent edges validates and plans, so it must
//!   start;
//! - a subgraph node whose start is refused must leave its child run wound down, not abandoned.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeRole, OverloadPolicy, PipelineId, PipelineSpec,
    PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{RunController, RunError, SubgraphNode};
use inference_testkit::graph::{PipelinePayload, TextSink};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

const PATIENCE: Duration = Duration::from_secs(10);

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

struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

fn catalog() -> Profiles {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    Profiles(profile)
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    }
}

fn n(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("positive")
}

/// Numbered envelopes of one stream, stamped with `run_id` and `producer`.
struct Stream {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Stream {
    fn new(run_id: RunId, producer: NodeId) -> Self {
        Self {
            run_id,
            producer,
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    fn envelope(
        &mut self,
        kind: String,
        payload: PipelinePayload,
    ) -> EventEnvelope<PipelinePayload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: "p".to_owned(),
            kind,
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }

    fn text(&mut self, text: &str) -> EventEnvelope<PipelinePayload> {
        let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: text.to_owned(),
        });
        self.envelope(event.name().to_owned(), PipelinePayload::Stream(event))
    }

    fn stop(&mut self) -> EventEnvelope<PipelinePayload> {
        self.envelope(
            ControlKind::Stop.as_str().to_owned(),
            PipelinePayload::Control(ControlKind::Stop),
        )
    }
}

/// A child node that, once started, sends `count` texts on its one output from a thread of its
/// own and holds nothing else; every other step answers at once.
struct Burst {
    run_id: RunId,
    count: usize,
}

impl NodeImplementation<PipelinePayload> for Burst {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn start(
        &mut self,
        mut endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let output = endpoints.outputs.remove(0).endpoint;
        let (run_id, count) = (self.run_id, self.count);
        thread::spawn(move || {
            let mut stream = Stream::new(run_id, NodeId::generate());
            for index in 0..count {
                if block_on(output.send(stream.text(&index.to_string()))).is_err() {
                    return;
                }
            }
        });
        Box::pin(std::future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}

/// Set once a node's thread has ended; a task polling it is woken then.
#[derive(Clone, Default)]
struct Done(Arc<Mutex<(bool, Option<Waker>)>>);

impl Done {
    fn set(&self) {
        let waker = {
            let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            state.0 = true;
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll(&self, cx: &Context<'_>) -> Poll<()> {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.0 {
            Poll::Ready(())
        } else {
            state.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// A child node that, for each text it takes on its one input, sends two texts on its one output,
/// one after the other, and forwards `stop`; it ends with its input. Drain waits for that end.
struct Echo {
    run_id: RunId,
    done: Done,
}

impl NodeImplementation<PipelinePayload> for Echo {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn start(
        &mut self,
        mut endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let mut input = endpoints.inputs.remove(0).endpoint;
        let output = endpoints.outputs.remove(0).endpoint;
        let run_id = self.run_id;
        let done = self.done.clone();
        thread::spawn(move || {
            let mut stream = Stream::new(run_id, NodeId::generate());
            while let Some(envelope) = block_on(input.recv()) {
                if envelope.kind == ControlKind::Stop.as_str() {
                    let _sent = block_on(output.send(stream.stop()));
                    break;
                }
                for half in ["a", "b"] {
                    if block_on(output.send(stream.text(half))).is_err() {
                        done.set();
                        return;
                    }
                }
            }
            done.set();
        });
        Box::pin(std::future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let done = self.done.clone();
        Box::pin(async move {
            std::future::poll_fn(|cx| done.poll(cx)).await;
            Ok(())
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}

/// A child of one node `relay` of role `role`, with the input `in` (when `input` is set) and the
/// output `out`, exposed under the same names with the capacities and policies given; planned.
fn child(
    declaration: &mut Declaration,
    catalog: &Profiles,
    role: NodeRole,
    input: Option<(usize, OverloadPolicy)>,
    output: (usize, OverloadPolicy),
) -> (PipelineId, PlannedPipeline) {
    let (child_id, ()) = declaration
        .build(catalog, catalog.0.profile_id(), spec("child"), |pipeline| {
            let relay = pipeline.node("relay", role, Vec::new());
            if let Some((capacity, overload)) = input {
                let into = pipeline.input::<modality::Text>(&relay, "in", TEXT);
                pipeline.expose(&into, "in", n(capacity), overload);
            }
            let out = pipeline.output::<modality::Text>(&relay, "out", TEXT);
            pipeline.expose(&out, "out", n(output.0), output.1);
        })
        .expect("the profile is declared");
    let validated = declaration.validate(child_id).expect("the child is valid");
    let planned = declaration
        .plan(validated, catalog)
        .expect("the child plans");
    (child_id, planned)
}

/// Spawns the loop of `controller` and answers its end, or none when it has not ended within
/// `patience`.
fn drive_within(
    mut controller: RunController<PipelinePayload>,
    patience: Duration,
) -> Option<(RunState, Result<(), RunError>)> {
    let (ended, answer) = mpsc::channel();
    thread::spawn(move || {
        let result = block_on(controller.drive());
        let _ = ended.send((controller.run().state(), result));
    });
    answer.recv_timeout(patience).ok()
}

#[test]
fn a_parent_cancelled_while_its_caller_does_not_read_the_egress_fed_by_the_subgraph_ends() {
    let catalog = catalog();
    let mut declaration = Declaration::new();
    let (child_id, child) = child(
        &mut declaration,
        &catalog,
        NodeRole::Source,
        None,
        (1, OverloadPolicy::Pause),
    );
    let (parent_id, subgraph) = declaration
        .build(
            &catalog,
            catalog.0.profile_id(),
            spec("parent"),
            |pipeline| {
                let frontend = pipeline
                    .subgraph("frontend", NodeRole::Transform, child_id)
                    .expect("the child is Planned");
                let out = pipeline.output::<modality::Text>(&frontend, "out", TEXT);
                pipeline.expose(&out, "reply", n(1), OverloadPolicy::Pause);
                frontend.id()
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(parent_id)
        .expect("the parent is valid");
    let parent = declaration
        .plan(validated, &catalog)
        .expect("the subgraph node matches the child's exposures");

    let run_id = RunId::generate();
    let mut node = Some(SubgraphNode::new(
        run_id,
        &declaration,
        &child,
        HashMap::<GenerationId, i64>::new(),
        Box::new(|run_id, _: &Node| {
            Box::new(Burst { run_id, count: 8 }) as Box<dyn NodeImplementation<PipelinePayload>>
        }),
    ));
    let mut controller = RunController::create_as(
        run_id,
        &parent,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |declared: &Node| {
            assert_eq!(declared.node_id(), subgraph);
            Box::new(node.take().expect("one node")) as Box<dyn NodeImplementation<PipelinePayload>>
        },
    )
    .expect("planned");
    let mut ends = controller.exposed();
    let egress = ends
        .egress("reply")
        .expect("the caller's end of the egress");
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("the child prepares");
    block_on(controller.activate()).expect("the child starts");
    let requests = controller.requests();
    thread::sleep(Duration::from_millis(200));
    requests.cancel();

    let ended = drive_within(controller, PATIENCE);
    drop(egress);
    let Some((state, result)) = ended else {
        panic!(
            "the parent's cancel did not return within {PATIENCE:?}: the subgraph node's egress \
             bridge waits on the full parent egress and its stop waits on the bridge"
        );
    };
    result.expect("the loop cancels the run");
    assert_eq!(state, RunState::Cancelled);
}

#[test]
fn a_child_ingress_under_fail_does_not_stall_the_childs_egress_bridge() {
    let catalog = catalog();
    let mut declaration = Declaration::new();
    let (child_id, child) = child(
        &mut declaration,
        &catalog,
        NodeRole::Transform,
        Some((1, OverloadPolicy::Fail)),
        (1, OverloadPolicy::Pause),
    );
    let (parent_id, ()) = declaration
        .build(
            &catalog,
            catalog.0.profile_id(),
            spec("parent"),
            |pipeline| {
                let frontend = pipeline
                    .subgraph("frontend", NodeRole::Transform, child_id)
                    .expect("the child is Planned");
                let into = pipeline.input::<modality::Text>(&frontend, "in", TEXT);
                let out = pipeline.output::<modality::Text>(&frontend, "out", TEXT);
                pipeline.expose(&into, "in", n(8), OverloadPolicy::Pause);
                pipeline.expose(&out, "out", n(64), OverloadPolicy::Pause);
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(parent_id)
        .expect("the parent is valid");
    let parent = declaration
        .plan(validated, &catalog)
        .expect("the subgraph node matches the child's exposures");

    let run_id = RunId::generate();
    let mut node = Some(SubgraphNode::new(
        run_id,
        &declaration,
        &child,
        HashMap::<GenerationId, i64>::new(),
        Box::new(|run_id, _: &Node| {
            Box::new(Echo {
                run_id,
                done: Done::default(),
            }) as Box<dyn NodeImplementation<PipelinePayload>>
        }),
    ));
    let mut controller = RunController::create_as(
        run_id,
        &parent,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| {
            Box::new(node.take().expect("one node")) as Box<dyn NodeImplementation<PipelinePayload>>
        },
    )
    .expect("planned");
    let mut ends = controller.exposed();
    let ingress = ends.ingress("in").expect("the caller's end of the ingress");
    let mut egress = ends.egress("out").expect("the caller's end of the egress");
    let mut caller = Stream::new(run_id, NodeId::generate());
    for text in ["1", "2", "3"] {
        block_on(ingress.send(caller.text(text))).expect("the parent ingress has room");
    }
    block_on(ingress.send(caller.stop())).expect("the parent ingress has room");
    let (read, received) = mpsc::channel();
    thread::spawn(move || {
        let mut texts = 0;
        while let Some(envelope) = block_on(egress.recv()) {
            if envelope.kind != ControlKind::Stop.as_str() {
                texts += 1;
            }
        }
        let _ = read.send(texts);
    });
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("the child prepares");
    block_on(controller.activate()).expect("the child starts");

    let ended = drive_within(controller, PATIENCE);
    let Some((state, result)) = ended else {
        panic!(
            "the parent did not complete within {PATIENCE:?}: the ingress bridge retries the full \
             `fail` ingress in a loop that never yields, so the egress bridge on the same thread \
             never takes the reply the child is blocked on"
        );
    };
    result.expect("the loop completes the run");
    assert_eq!(state, RunState::Completed);
    assert_eq!(
        received.recv_timeout(PATIENCE).ok(),
        Some(6),
        "two texts per input"
    );
    drop(ingress);
}

/// A parent whose subgraph node over a child with the input `in` and output `out` feeds two
/// sinks from its output port, the child run on `relay`; validated, planned and prepared.
fn fanned_out(
    relay: impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<PipelinePayload>> + Send + 'static,
) -> RunController<PipelinePayload> {
    let catalog = catalog();
    let mut declaration = Declaration::new();
    let (child_id, child) = child(
        &mut declaration,
        &catalog,
        NodeRole::Transform,
        Some((4, OverloadPolicy::Pause)),
        (4, OverloadPolicy::Pause),
    );
    let (parent_id, frontend) = declaration
        .build(
            &catalog,
            catalog.0.profile_id(),
            spec("parent"),
            |pipeline| {
                let frontend = pipeline
                    .subgraph("frontend", NodeRole::Transform, child_id)
                    .expect("the child is Planned");
                let into = pipeline.input::<modality::Text>(&frontend, "in", TEXT);
                let out = pipeline.output::<modality::Text>(&frontend, "out", TEXT);
                pipeline.expose(&into, "in", n(4), OverloadPolicy::Pause);
                for name in ["left", "right"] {
                    let sink = pipeline.node(name, NodeRole::Sink, Vec::new());
                    let text = pipeline.input::<modality::Text>(&sink, "text", TEXT);
                    pipeline.connect(&out, &text, n(4), OverloadPolicy::Pause);
                }
                frontend.id()
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(parent_id)
        .expect("an output feeding two edges is valid");
    let parent = declaration
        .plan(validated, &catalog)
        .expect("the subgraph node matches the child's exposures");
    let run_id = RunId::generate();
    let mut subgraph = Some(SubgraphNode::new(
        run_id,
        &declaration,
        &child,
        HashMap::<GenerationId, i64>::new(),
        Box::new(relay),
    ));
    let mut controller = RunController::create_as(
        run_id,
        &parent,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            if node.node_id() == frontend {
                Box::new(subgraph.take().expect("one subgraph node"))
                    as Box<dyn NodeImplementation<PipelinePayload>>
            } else {
                Box::new(TextSink::new())
            }
        },
    )
    .expect("planned");
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("the child prepares");
    controller
}

#[test]
fn a_subgraph_node_whose_output_feeds_two_edges_starts() {
    let mut controller = fanned_out(|run_id, _| {
        Box::new(Echo {
            run_id,
            done: Done::default(),
        })
    });
    let started = block_on(controller.activate());
    assert_eq!(
        started,
        Ok(()),
        "a parent that validated and planned starts its subgraph node"
    );
    controller.requests().cancel();
    let _ = drive_within(controller, PATIENCE);
}

type Received = Arc<Mutex<Vec<EventEnvelope<PipelinePayload>>>>;

/// A child node that keeps every envelope its one input takes, until `stop` or the input's end.
struct Recorder {
    received: Received,
    done: Done,
}

impl NodeImplementation<PipelinePayload> for Recorder {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn start(
        &mut self,
        mut endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let mut input = endpoints.inputs.remove(0).endpoint;
        let (received, done) = (Arc::clone(&self.received), self.done.clone());
        thread::spawn(move || {
            while let Some(envelope) = block_on(input.recv()) {
                let end = envelope.kind == ControlKind::Stop.as_str();
                received
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(envelope);
                if end {
                    break;
                }
            }
            done.set();
        });
        Box::pin(std::future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let done = self.done.clone();
        Box::pin(async move {
            std::future::poll_fn(|cx| done.poll(cx)).await;
            Ok(())
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}

#[test]
fn an_envelope_crossing_into_the_child_carries_the_child_run_and_names_its_parent_counterpart() {
    let catalog = catalog();
    let mut declaration = Declaration::new();
    let (child_id, ()) = declaration
        .build(
            &catalog,
            catalog.0.profile_id(),
            spec("child"),
            |pipeline| {
                let recorder = pipeline.node("recorder", NodeRole::Sink, Vec::new());
                let into = pipeline.input::<modality::Text>(&recorder, "in", TEXT);
                pipeline.expose(&into, "in", n(4), OverloadPolicy::Pause);
            },
        )
        .expect("the profile is declared");
    let validated = declaration.validate(child_id).expect("the child is valid");
    let child = declaration
        .plan(validated, &catalog)
        .expect("the child plans");
    let (parent_id, frontend) = declaration
        .build(
            &catalog,
            catalog.0.profile_id(),
            spec("parent"),
            |pipeline| {
                let frontend = pipeline
                    .subgraph("frontend", NodeRole::Sink, child_id)
                    .expect("the child is Planned");
                let into = pipeline.input::<modality::Text>(&frontend, "in", TEXT);
                pipeline.expose(&into, "in", n(4), OverloadPolicy::Pause);
                frontend.id()
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(parent_id)
        .expect("the parent is valid");
    let parent = declaration
        .plan(validated, &catalog)
        .expect("the subgraph node matches the child's exposures");

    let received = Received::default();
    let child_received = Arc::clone(&received);
    let run_id = RunId::generate();
    let subgraph = SubgraphNode::new(
        run_id,
        &declaration,
        &child,
        HashMap::<GenerationId, i64>::new(),
        Box::new(move |_, _: &Node| {
            Box::new(Recorder {
                received: Arc::clone(&child_received),
                done: Done::default(),
            }) as Box<dyn NodeImplementation<PipelinePayload>>
        }),
    );
    let handle = subgraph.handle();
    let mut subgraph = Some(subgraph);
    let mut controller = RunController::create_as(
        run_id,
        &parent,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| {
            Box::new(subgraph.take().expect("one node"))
                as Box<dyn NodeImplementation<PipelinePayload>>
        },
    )
    .expect("planned");
    let mut ends = controller.exposed();
    let ingress = ends.ingress("in").expect("the caller's end of the ingress");
    let mut caller = Stream::new(run_id, NodeId::generate());
    let sent = caller.text("hello");
    let sent_id = sent.event_id;
    block_on(ingress.send(sent)).expect("room");
    block_on(ingress.send(caller.stop())).expect("room");
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("the child prepares");
    block_on(controller.activate()).expect("the child starts");
    let Some((state, result)) = drive_within(controller, PATIENCE) else {
        panic!("the parent did not complete within {PATIENCE:?}");
    };
    result.expect("the loop completes the run");
    assert_eq!(state, RunState::Completed);

    let child_run = handle.child_run_id().expect("the child run was created");
    let received = received
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let text = received
        .iter()
        .find(|envelope| envelope.kind != ControlKind::Stop.as_str())
        .expect("the text crossed into the child");
    assert_eq!(
        text.run_id, child_run,
        "stamped with the child run's identity"
    );
    assert_eq!(
        text.causation_id,
        Some(sent_id),
        "caused by the parent-side envelope it crossed for"
    );
    assert_eq!(text.producer, frontend, "the subgraph node emitted it");
    assert_ne!(text.event_id, sent_id, "a new event identity");
    assert_eq!(text.port, "in", "the exposure it crossed to");
    drop(ingress);
}
