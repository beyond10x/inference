//! §23.1 test 6 of `docs/design/inference.md` (line 1110): event IDs, generation IDs and
//! per-stream sequence constraints are enforced. [`EventLog`] records `inference.stream` events
//! in their `inference.stream.EventEnvelope` and refuses, one test per rule, a second event
//! carrying an `event_id` already recorded, an event whose `sequence` is not greater than the last
//! one recorded on its `stream_id`, and an event whose payload carries a `generation_id` while the
//! envelope carries none or a different one. A refused event is not recorded.

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::generation::GenerationCompleted;
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_testkit::stream::{EnvelopeRefusal, EventLog, StreamEvent};
use inference_text::{SegmentId, SegmentQueued, TextDelta};
use uuid::Uuid;

fn id(low: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | low)
}

fn generation(low: u128) -> GenerationId {
    GenerationId::from(id(0x6000 | low))
}

fn stream(low: u128) -> StreamId {
    StreamId::from(id(0x5000 | low))
}

/// A `TextDelta` of `generation(1)`, enveloped as event `event` at `sequence` on `stream(1)` with
/// the generation the payload carries.
fn delta(event: u128, sequence: i64) -> EventEnvelope<StreamEvent> {
    let payload = StreamEvent::TextDelta(TextDelta {
        segment_id: SegmentId::from(id(0x7000 | event)),
        generation_id: generation(1),
        sequence,
        text: format!("t{sequence}"),
    });
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::from(id(event)),
        run_id: RunId::from(id(0x2000)),
        session_id: None,
        stream_id: stream(1),
        sequence,
        producer: NodeId::from(id(0x3000)),
        port: "text_out".to_owned(),
        kind: payload.name().to_owned(),
        generation_id: Some(generation(1)),
        timestamp: sequence,
        timebase: Timebase::InferenceExecutionTime,
        causation_id: None,
        payload,
    }
}

#[test]
fn a_second_event_carrying_a_recorded_event_id_is_refused() {
    let mut log = EventLog::new();
    log.record(delta(1, 1)).expect("a first event is recorded");

    let mut again = delta(1, 2);
    again.stream_id = stream(2);
    assert_eq!(
        log.record(again),
        Err(EnvelopeRefusal::DuplicateEventId {
            event_id: EventId::from(id(1))
        }),
        "the same event id on another stream and a later sequence is still a duplicate"
    );
    assert_eq!(log.len(), 1, "a refused event is not recorded");

    log.record(delta(2, 2)).expect("a new event id is recorded");
    assert_eq!(log.len(), 2);
}

#[test]
fn an_event_whose_sequence_is_not_greater_than_the_last_on_its_stream_is_refused() {
    let mut log = EventLog::new();
    log.record(delta(1, 5)).expect("a first event is recorded");

    for (event, sequence) in [(11, 5), (12, 4), (13, i64::MIN)] {
        assert_eq!(
            log.record(delta(event, sequence)),
            Err(EnvelopeRefusal::SequenceNotIncreasing {
                stream_id: stream(1),
                last: 5,
                sequence,
            }),
            "sequence {sequence} after 5 on the same stream"
        );
    }
    assert_eq!(log.len(), 1, "a refused event is not recorded");

    let mut other = delta(2, 1);
    other.stream_id = stream(2);
    log.record(other)
        .expect("another stream is ordered on its own: no global order (line 320)");
    log.record(delta(3, 6))
        .expect("a greater sequence on the first stream is recorded");
    assert_eq!(
        log.record(delta(4, 6)),
        Err(EnvelopeRefusal::SequenceNotIncreasing {
            stream_id: stream(1),
            last: 6,
            sequence: 6,
        }),
        "the last recorded sequence moved to 6"
    );
    assert_eq!(log.len(), 3);
}

#[test]
fn a_stream_event_whose_payload_generation_differs_from_the_envelope_is_refused() {
    let mut log = EventLog::new();

    let mut none = delta(1, 1);
    none.generation_id = None;
    assert_eq!(
        log.record(none),
        Err(EnvelopeRefusal::GenerationMismatch {
            payload: generation(1),
            envelope: None,
        }),
        "the payload carries a generation and the envelope none"
    );

    let mut other = delta(2, 1);
    other.generation_id = Some(generation(2));
    assert_eq!(
        log.record(other),
        Err(EnvelopeRefusal::GenerationMismatch {
            payload: generation(1),
            envelope: Some(generation(2)),
        }),
        "the envelope carries another generation"
    );

    let mut completed = delta(3, 1);
    completed.payload = StreamEvent::GenerationCompleted(GenerationCompleted {
        generation_id: generation(3),
    });
    completed.kind = completed.payload.name().to_owned();
    assert_eq!(
        log.record(completed),
        Err(EnvelopeRefusal::GenerationMismatch {
            payload: generation(3),
            envelope: Some(generation(1)),
        }),
        "a generation event carries its generation in the payload too"
    );
    assert!(log.is_empty(), "a refused event is not recorded");

    log.record(delta(4, 1))
        .expect("the same generation in payload and envelope is recorded");
    let mut queued = delta(5, 2);
    queued.payload = StreamEvent::SegmentQueued(SegmentQueued {
        segment_id: SegmentId::from(id(0x7004)),
    });
    queued.kind = queued.payload.name().to_owned();
    log.record(queued)
        .expect("a payload that carries no generation constrains none");
    assert_eq!(log.len(), 2);
}
