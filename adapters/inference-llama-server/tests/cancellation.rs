//! Cancellation, failures, deadlines and disconnects of the adapter, against the misbehaving
//! loopback fixture of `faults`. No server is started and no weights are read.
//!
//! Cancelling delivery and asking the server to stop computing are two facts, and separate tests
//! report them. Delivery cancellation is `GenerationCancelled` with no later delta, though the
//! rest of the stream had reached the adapter; the usage record of a turn that ended before the
//! cancel is still delivered. The request to stop computing is the connection
//! closed while the fixture still held the rest of the stream: `llama-server` stops a task whose
//! connection is gone. Whether a live server did stop is the live test's to observe.
//!
//! Every case ends by giving back what its generation held: the handle dropped, no thread of its
//! turn still running ([`LlamaServer::running_turns`]), and a new generation on the same generator,
//! whose maximum concurrency is 1, starting and completing against the same fixture, which serves
//! one connection at a time.

mod common;
mod faults;

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use common::{DELTAS, SERVED_MODEL};
use faults::{Ending, Faults, Script};
use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId};
use inference_core::model::{Adapter, Model};
use inference_llama_server::{Config, LlamaServer, declare_adapter, declare_model};
use inference_llm::llm_core::{ErrorCode, Item, TurnRequest};
use inference_llm::{GenerationEvent, GenerationHandle, GenerationRequest, Generator};
use inference_text::{SegmentDiscarded, SegmentState};

/// The longest a case waits for something that happens at once when nothing leaks.
const WITHIN: Duration = Duration::from_secs(5);

/// The idle limit of the deadline case: far below `WITHIN`, far above a loopback round trip.
const IDLE: Duration = Duration::from_millis(300);

/// The model of a server at `faults`, with `idle` as its idle limit when given, and a generator
/// of one slot over it; the test keeps the model to count its turns.
fn subject(faults: &Faults, idle: Option<Duration>) -> (Arc<LlamaServer>, Generator) {
    let mut config = Config::new(faults.base_url(), SERVED_MODEL);
    if let Some(idle) = idle {
        config.limits.idle = idle;
    }
    let (model, _) = Model::declare(declare_model());
    let (adapter, _) =
        Adapter::declare(declare_adapter(model.model_id(), &config)).expect("one slot");
    assert_eq!(adapter.max_concurrency(), 1);
    let server = Arc::new(LlamaServer::new(&config).expect("the fixture's base URL binds"));
    let generator = Generator::new(adapter, Arc::clone(&server) as _)
        .expect("the binding declares what the adapter claims");
    (server, generator)
}

fn start(generator: &mut Generator) -> GenerationHandle {
    generator
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn: TurnRequest::new(SERVED_MODEL, vec![Item::user("Say hello in three words.")]),
        })
        .expect("the generator's one slot is free")
}

/// Takes events up to and including the first text delta.
fn until_first_delta(handle: &mut GenerationHandle) -> Vec<GenerationEvent> {
    let mut events = Vec::new();
    for event in handle.by_ref() {
        let delta = matches!(event, GenerationEvent::TextDelta(_));
        events.push(event);
        if delta {
            return events;
        }
    }
    panic!("the generation ended before a delta: {events:?}")
}

fn names(events: &[GenerationEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            GenerationEvent::Started(_) => "StartGeneration",
            GenerationEvent::TextDelta(_) => "EmitTextDelta",
            GenerationEvent::SegmentCommitted(_) => "CommitSegment",
            GenerationEvent::Usage(_) => "RecordUsage",
            GenerationEvent::Completed(_) => "CompleteGeneration",
            GenerationEvent::Cancelled(..) => "CancelGeneration",
        })
        .collect()
}

/// Waits until no thread of a turn of `server` runs.
fn turns_end(server: &LlamaServer) {
    let deadline = Instant::now() + WITHIN;
    while server.running_turns() > 0 {
        assert!(
            Instant::now() < deadline,
            "{} thread(s) of a turn still run {WITHIN:?} after its generation ended",
            server.running_turns()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

/// Drops `handle` and checks that its generation gave back what it held: no thread of its turn
/// runs, no connection was left for the fixture to give up on, and a new generation on the same
/// generator starts in the one slot and completes on the next connection, in time.
fn assert_released(
    server: &LlamaServer,
    generator: &mut Generator,
    faults: &Faults,
    handle: GenerationHandle,
) {
    drop(handle);
    turns_end(server);
    let earlier = faults.bodies().len();
    for connection in 0..earlier {
        let ending = faults.ending(connection, WITHIN);
        assert!(
            matches!(ending, Some(Ending::Served | Ending::ClosedByClient)),
            "connection {connection} was let go: {ending:?}"
        );
    }

    let began = Instant::now();
    let events: Vec<GenerationEvent> = start(generator).collect();
    assert!(
        matches!(events.last(), Some(GenerationEvent::Completed(_))),
        "the next generation completes: {events:?}"
    );
    assert!(
        began.elapsed() < WITHIN,
        "and in time: {:?}",
        began.elapsed()
    );
    assert_eq!(faults.ending(earlier, WITHIN), Some(Ending::Served));
    turns_end(server);
}

#[test]
fn cancelling_after_the_first_delta_delivers_generation_cancelled_and_no_later_delta() {
    let faults = Faults::new(&[Script::Replay]);
    let (server, mut generator) = subject(&faults, None);
    let mut handle = start(&mut generator);
    let first = until_first_delta(&mut handle);
    assert_eq!(names(&first), ["StartGeneration", "EmitTextDelta"]);
    let GenerationEvent::TextDelta(delivered) = &first[1] else {
        unreachable!("the last event taken is a delta")
    };
    assert_eq!(
        faults.ending(0, WITHIN),
        Some(Ending::Served),
        "the fixture sent the whole stream before the cancel"
    );
    turns_end(&server);

    let generation_id = handle.output().generation().generation_id();
    let cancelled = handle.cancel().expect("the generation is Active");
    assert_eq!(cancelled.generation_id, generation_id);
    assert_eq!(
        handle.output().generation().state(),
        GenerationState::Cancelled
    );
    let later: Vec<GenerationEvent> = handle.by_ref().collect();
    assert!(
        matches!(later.as_slice(), [GenerationEvent::Usage(recorded)] if recorded.r#final),
        "none of the {} later deltas the adapter received is delivered, only the usage record \
         of the turn, which ended before the cancel: {later:?}",
        DELTAS.len() - 1
    );
    assert!(handle.next().is_none());
    assert!(
        handle.failure().is_none(),
        "a cancel by the caller is no failure: {:?}",
        handle.failure()
    );
    let segments = handle.output().segments();
    assert_eq!(
        segments.first().map(|segment| segment.segment_id()),
        Some(delivered.segment_id),
        "the delivered delta is the generation's first segment"
    );
    assert!(
        segments
            .iter()
            .all(|segment| segment.state() == SegmentState::Discarded),
        "every segment is discarded, the delivered one and any the handle took but never \
         delivered: {segments:?}"
    );

    assert_released(&server, &mut generator, &faults, handle);
}

#[test]
fn cancelling_after_the_first_delta_closes_the_connection_before_the_rest_of_the_stream() {
    let faults = Faults::new(&[Script::Stall]);
    let (server, mut generator) = subject(&faults, None);
    let mut handle = start(&mut generator);
    until_first_delta(&mut handle);
    assert_eq!(server.running_turns(), 1, "the turn runs, the stream open");

    handle.cancel().expect("the generation is Active");
    assert_eq!(
        faults.ending(0, WITHIN),
        Some(Ending::ClosedByClient),
        "the adapter asks the server to stop computing by closing the connection, \
         while the server still holds the rest of the stream"
    );
    turns_end(&server);

    assert_released(&server, &mut generator, &faults, handle);
}

#[test]
fn a_loading_server_answering_503_cancels_the_generation_with_no_segment_and_keeps_the_error() {
    let faults = Faults::new(&[Script::Loading]);
    let (server, mut generator) = subject(&faults, None);
    let mut handle = start(&mut generator);
    let events: Vec<GenerationEvent> = handle.by_ref().collect();

    assert_eq!(
        names(&events),
        ["StartGeneration", "RecordUsage", "CancelGeneration"],
        "{events:?}"
    );
    let Some(GenerationEvent::Cancelled(cancelled, discarded)) = events.last() else {
        unreachable!("the last event is the cancel")
    };
    assert_eq!(
        cancelled.generation_id,
        handle.output().generation().generation_id()
    );
    assert!(discarded.is_empty(), "no segment to discard: {discarded:?}");
    assert!(handle.output().segments().is_empty());
    let failure = handle.failure().expect("the llm error is kept");
    assert_eq!(failure.code, ErrorCode::Transport, "{failure:?}");
    assert!(failure.message.contains("503"), "{failure:?}");
    assert!(handle.next().is_none(), "no event after the cancel");

    assert_released(&server, &mut generator, &faults, handle);
}

#[test]
fn a_stream_cut_after_one_delta_cancels_the_generation_discarding_that_segment() {
    let faults = Faults::new(&[Script::Cut]);
    let (server, mut generator) = subject(&faults, None);
    let mut handle = start(&mut generator);
    let events: Vec<GenerationEvent> = handle.by_ref().collect();

    assert_eq!(
        names(&events),
        [
            "StartGeneration",
            "EmitTextDelta",
            "CommitSegment",
            "RecordUsage",
            "CancelGeneration"
        ],
        "{events:?}"
    );
    let GenerationEvent::TextDelta(delta) = &events[1] else {
        unreachable!("the second event is the delta")
    };
    assert_eq!(delta.text, DELTAS[0]);
    let Some(GenerationEvent::Cancelled(_, discarded)) = events.last() else {
        unreachable!("the last event is the cancel")
    };
    assert_eq!(
        discarded,
        &[SegmentDiscarded {
            segment_id: delta.segment_id
        }],
        "the segment of the delta is among the discarded"
    );
    let failure = handle.failure().expect("the llm error is kept");
    assert_eq!(failure.code, ErrorCode::Protocol, "{failure:?}");
    assert!(handle.next().is_none(), "no event after the cancel");

    assert_released(&server, &mut generator, &faults, handle);
}

#[test]
fn a_server_silent_past_the_idle_limit_cancels_the_generation_with_the_deadline_error() {
    let faults = Faults::new(&[Script::Stall]);
    let (server, mut generator) = subject(&faults, Some(IDLE));
    let mut handle = start(&mut generator);
    let began = Instant::now();
    let events: Vec<GenerationEvent> = handle.by_ref().collect();
    let waited = began.elapsed();

    assert_eq!(
        names(&events),
        [
            "StartGeneration",
            "EmitTextDelta",
            "CommitSegment",
            "RecordUsage",
            "CancelGeneration"
        ],
        "{events:?}"
    );
    assert!(
        waited >= IDLE && waited < WITHIN,
        "the idle limit ended the wait: {waited:?}"
    );
    let failure = handle.failure().expect("the llm error is kept");
    assert_eq!(failure.code, ErrorCode::Deadline, "{failure:?}");
    assert_eq!(
        faults.ending(0, WITHIN),
        Some(Ending::ClosedByClient),
        "the stalled connection is closed with the generation"
    );
    assert!(handle.next().is_none(), "no event after the cancel");

    assert_released(&server, &mut generator, &faults, handle);
}

#[test]
fn dropping_a_running_generation_closes_its_connection_and_gives_back_its_slot() {
    let faults = Faults::new(&[Script::Stall]);
    let (server, mut generator) = subject(&faults, None);
    let mut handle = start(&mut generator);
    until_first_delta(&mut handle);
    assert_eq!(server.running_turns(), 1, "the turn runs, the stream open");

    assert_released(&server, &mut generator, &faults, handle);
    assert_eq!(faults.ending(0, WITHIN), Some(Ending::ClosedByClient));
}
