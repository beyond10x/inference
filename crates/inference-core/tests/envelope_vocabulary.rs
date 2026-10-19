//! `inference.stream.EventEnvelope`, `inference.stream.Timebase` and `inference.stream.EventId`
//! follow `ess/domains/stream.yaml`: every field of the envelope is written under its ESS name,
//! the three optional ones as `null` when absent, the timebase as one of the four ESS clock names,
//! and the event id as the identifiers of `inference_core::id` are.

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, SessionId, StreamId};
use serde_json::{Value, json};
use uuid::Uuid;

fn id(low: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | low)
}

fn envelope() -> EventEnvelope<Value> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::from(id(1)),
        run_id: RunId::from(id(2)),
        session_id: Some(SessionId::from(id(3))),
        stream_id: StreamId::from(id(4)),
        sequence: 7,
        producer: NodeId::from(id(5)),
        port: "text_out".to_owned(),
        kind: "inference.stream.TextDelta".to_owned(),
        generation_id: Some(GenerationId::from(id(6))),
        timestamp: 1_500,
        timebase: Timebase::InferenceExecutionTime,
        causation_id: Some(EventId::from(id(8))),
        payload: json!({"text": "hello"}),
    }
}

#[test]
fn timebase_all_serializes_to_the_four_ess_clock_names_in_order() {
    assert_eq!(
        serde_json::to_value(Timebase::ALL).expect("serializes"),
        json!([
            "audio_frame_time",
            "inference_execution_time",
            "wall_time",
            "playback_time",
        ])
    );
    for timebase in Timebase::ALL {
        let wire = serde_json::to_value(timebase).expect("serializes");
        assert_eq!(
            serde_json::from_value::<Timebase>(wire).expect("deserializes"),
            timebase
        );
    }
    assert!(serde_json::from_value::<Timebase>(json!("WallTime")).is_err());
}

#[test]
fn an_envelope_writes_every_field_under_its_ess_name_and_reads_itself_back() {
    let envelope = envelope();
    let wire = serde_json::to_value(&envelope).expect("serializes");
    assert_eq!(
        wire,
        json!({
            "schema_version": 1,
            "event_id": "00000000-0000-4000-8000-000000000001",
            "run_id": "00000000-0000-4000-8000-000000000002",
            "session_id": "00000000-0000-4000-8000-000000000003",
            "stream_id": "00000000-0000-4000-8000-000000000004",
            "sequence": 7,
            "producer": "00000000-0000-4000-8000-000000000005",
            "port": "text_out",
            "kind": "inference.stream.TextDelta",
            "generation_id": "00000000-0000-4000-8000-000000000006",
            "timestamp": 1_500,
            "timebase": "inference_execution_time",
            "causation_id": "00000000-0000-4000-8000-000000000008",
            "payload": {"text": "hello"},
        })
    );
    let back: EventEnvelope<Value> = serde_json::from_value(wire).expect("deserializes");
    assert_eq!(back, envelope);
}

#[test]
fn session_generation_and_causation_are_optional_and_nothing_else_is() {
    let bare = EventEnvelope {
        session_id: None,
        generation_id: None,
        causation_id: None,
        ..envelope()
    };
    let wire = serde_json::to_value(&bare).expect("serializes");
    for optional in ["session_id", "generation_id", "causation_id"] {
        assert_eq!(wire[optional], Value::Null, "{optional} is written as null");
        let mut missing = wire.clone();
        missing.as_object_mut().expect("an object").remove(optional);
        let back: EventEnvelope<Value> = serde_json::from_value(missing)
            .unwrap_or_else(|error| panic!("an envelope without {optional} is read: {error}"));
        assert_eq!(back, bare);
    }
    for required in [
        "schema_version",
        "event_id",
        "run_id",
        "stream_id",
        "sequence",
        "producer",
        "port",
        "kind",
        "timestamp",
        "timebase",
        "payload",
    ] {
        let mut missing = wire.clone();
        missing.as_object_mut().expect("an object").remove(required);
        assert!(
            serde_json::from_value::<EventEnvelope<Value>>(missing).is_err(),
            "an envelope without {required} is refused"
        );
    }
}

#[test]
fn an_event_id_reads_the_canonical_spelling_only() {
    let canonical = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
    let lower: EventId = serde_json::from_value(json!(canonical)).expect("canonical");
    let upper: EventId =
        serde_json::from_value(json!(canonical.to_uppercase())).expect("either case");
    assert_eq!(lower, upper);
    assert_eq!(
        serde_json::to_value(upper).expect("serializes"),
        json!(canonical)
    );
    for refused in [
        format!("urn:uuid:{canonical}"),
        format!("{{{canonical}}}"),
        canonical.replace('-', ""),
        format!(" {canonical}"),
    ] {
        assert!(
            serde_json::from_value::<EventId>(json!(refused)).is_err(),
            "{refused} is refused"
        );
    }
}
