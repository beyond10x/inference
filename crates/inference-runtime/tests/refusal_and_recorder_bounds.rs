//! A run's refusals and a recorder's entries are bounded
//! (`story:runtime-subgraph-and-recorder-bounds`):
//!
//! - a run keeps the first [`REFUSALS_KEPT`] refusals and counts the rest, so a node that keeps
//!   sending non-increasing sequences does not grow the run's memory, and reading the refusals
//!   copies no more than the cap;
//! - a recorder keeps at most its bound of entries in memory, the first it took, writes every
//!   entry to its sinks and counts what it did not keep.

use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use inference_core::endpoint::{EndpointFuture, OutputEndpoint, SendError};
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeRole, OverloadPolicy, PipelineSpec, PortTraits, modality,
};
use inference_runtime::record::{DEFAULT_RECORDER_BOUND, Recorded, Recorder};
use inference_runtime::{REFUSALS_KEPT, RunController};
use inference_testkit::graph::PipelinePayload;
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

/// A node that does nothing at any step.
struct Idle;

impl NodeImplementation<PipelinePayload> for Idle {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        _: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }
}

/// Polls `future` once; every send here completes on its first poll.
fn once<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the send completes at once"),
    }
}

fn text(run_id: RunId, stream_id: StreamId, sequence: i64) -> EventEnvelope<PipelinePayload> {
    let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: sequence.to_string(),
    });
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id,
        session_id: None,
        stream_id,
        sequence,
        producer: NodeId::generate(),
        port: "in".to_owned(),
        kind: event.name().to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: PipelinePayload::Stream(event),
    }
}

/// Acceptance: more refusals than the cap leave the list at the cap, the first ones kept, with
/// the overflow counted.
#[test]
fn more_refusals_than_the_cap_leave_the_list_at_the_cap_with_the_overflow_counted() {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let catalog = Profiles(profile);
    let mut declaration = Declaration::new();
    let spec = PipelineSpec {
        name: "sink".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let (pipeline_id, ()) = declaration
        .build(&catalog, catalog.0.profile_id(), spec, |pipeline| {
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let into = pipeline.input::<modality::Text>(&sink, "in", TEXT);
            pipeline.expose(
                &into,
                "in",
                NonZeroUsize::new(4).expect("4"),
                OverloadPolicy::Pause,
            );
        })
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline_id).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");
    let run_id = RunId::generate();
    let mut controller = RunController::create_as(
        run_id,
        &planned,
        &declaration,
        std::collections::HashMap::<GenerationId, i64>::new(),
        |_: &Node| Box::new(Idle) as Box<dyn NodeImplementation<PipelinePayload>>,
    )
    .expect("planned");
    let mut ends = controller.exposed();
    let ingress = ends.ingress("in").expect("the caller's end of the ingress");

    let stream_id = StreamId::generate();
    once(ingress.send(text(run_id, stream_id, 1))).expect("the first sequence is taken");
    let overflow = 10;
    let refused = REFUSALS_KEPT + overflow;
    for repeat in 0..refused {
        let answer = once(ingress.send(text(run_id, stream_id, 1)));
        assert!(
            matches!(
                answer,
                Err(SendError::SequenceNotIncreasing { last: 1, .. })
            ),
            "repeat {repeat} is refused"
        );
    }

    let refusals = controller.refusals();
    assert_eq!(
        refusals.len(),
        REFUSALS_KEPT,
        "story:runtime-subgraph-and-recorder-bounds: the run keeps at most {REFUSALS_KEPT} refusals"
    );
    assert!(
        refusals
            .iter()
            .all(|refusal| refusal.stream_id == stream_id && refusal.sequence == 1),
        "the kept refusals are the run's"
    );
    assert_eq!(
        controller.refusals_overflow(),
        u64::try_from(overflow).expect("small"),
        "every refusal past the cap is counted"
    );
    drop(ingress);
}

/// An ingress that takes everything at once.
struct Taking;

impl OutputEndpoint<()> for Taking {
    fn send(&self, _: EventEnvelope<()>) -> EndpointFuture<'_, Result<(), SendError<()>>> {
        Box::pin(future::ready(Ok(())))
    }
}

fn unit(sequence: i64) -> EventEnvelope<()> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence,
        producer: NodeId::generate(),
        port: "audio".to_owned(),
        kind: "inference.audio.AudioFrame".to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: (),
    }
}

/// Acceptance: a run recording more than the recorder's bound keeps memory within it and reports
/// the drop count; the sink still receives every entry, in merge order.
#[test]
fn a_recorder_past_its_bound_keeps_the_first_entries_and_counts_the_rest() {
    let written: Arc<Mutex<Vec<u64>>> = Arc::default();
    let sink = Arc::clone(&written);
    let recorder = Recorder::new("1")
        .with_sink(move |recorded: &Recorded<()>| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(recorded.merge_order);
        })
        .with_bound(4);
    assert_eq!(recorder.bound(), 4);
    let ingress = recorder.ingress("audio", Box::new(Taking));
    for sequence in 1..=10 {
        once(ingress.send(unit(sequence))).expect("taken");
    }
    let recording = recorder.recording();
    assert_eq!(
        recording
            .entries
            .iter()
            .map(|recorded| recorded.merge_order)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3],
        "story:runtime-subgraph-and-recorder-bounds: the recorder keeps its first 4 entries"
    );
    assert_eq!(
        recorder.dropped(),
        6,
        "the six entries past the bound are counted"
    );
    assert_eq!(
        *written.lock().unwrap_or_else(PoisonError::into_inner),
        (0..10).collect::<Vec<u64>>(),
        "the sink receives every entry"
    );
}

/// Acceptance: the default bound holds too: a recorder told no bound keeps
/// [`DEFAULT_RECORDER_BOUND`] entries.
#[test]
fn a_recorder_told_no_bound_keeps_the_default_bound() {
    let recorder = Recorder::new("1");
    assert_eq!(recorder.bound(), DEFAULT_RECORDER_BOUND);
    let ingress = recorder.ingress("audio", Box::new(Taking));
    let past = 3;
    for sequence in 1..=i64::try_from(DEFAULT_RECORDER_BOUND + past).expect("fits") {
        once(ingress.send(unit(sequence))).expect("taken");
    }
    assert_eq!(recorder.recording().entries.len(), DEFAULT_RECORDER_BOUND);
    assert_eq!(recorder.dropped(), u64::try_from(past).expect("small"));
}
