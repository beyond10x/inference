//! Adversary cases for `inference.stream.EventEnvelope` and `inference.stream.Timebase`, driven
//! from `ess/domains/stream.yaml` read as written rather than from literals: the envelope writes
//! exactly the declared fields in the declared order, a field is optional exactly when its declared
//! type is `Optional<…>`, an `Integer` field is a JSON integer, and the timebase serializes to the
//! declared variants in the declared order.

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, SessionId, StreamId};
use serde_json::{Value, json};
use uuid::Uuid;

const STREAM_YAML: &str = include_str!("../../../ess/domains/stream.yaml");

/// The lines of the `types:` entry named `name`, up to the next entry or section.
fn type_entry(name: &str) -> Vec<&'static str> {
    let mut lines = STREAM_YAML.lines();
    let header = format!("  - name: {name}");
    lines
        .by_ref()
        .find(|line| *line == header)
        .expect("declared");
    lines
        .filter(|line| !line.trim_start().starts_with('#') && !line.trim().is_empty())
        .take_while(|line| line.starts_with("    "))
        .collect()
}

/// The `fields:` of the struct `name`: (field, type), in the order they are written.
fn struct_fields(name: &str) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in type_entry(name) {
        let body = line.trim_start();
        if let Some(field) = body.strip_prefix("- name: ") {
            fields.push((field.to_owned(), String::new()));
        } else if let (Some(kind), Some(last)) = (body.strip_prefix("type: "), fields.last_mut()) {
            kind.clone_into(&mut last.1);
        }
    }
    fields
}

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
        port: "out".to_owned(),
        kind: "inference.stream.TextDelta".to_owned(),
        generation_id: Some(GenerationId::from(id(6))),
        timestamp: 1_500,
        timebase: Timebase::PlaybackTime,
        causation_id: Some(EventId::from(id(8))),
        payload: json!(0),
    }
}

#[test]
fn the_envelope_writes_the_declared_fields_in_order_and_is_optional_where_declared() {
    let declared = struct_fields("inference.stream.EventEnvelope");
    assert_eq!(declared.len(), 14, "{declared:?}");

    let written = serde_json::to_string(&envelope()).expect("serializes");
    let mut at = 0;
    for (field, _) in &declared {
        let key = format!("\"{field}\":");
        let found = written[at..]
            .find(&key)
            .unwrap_or_else(|| panic!("{field} is written after the field before it: {written}"));
        at += found + key.len();
    }
    let wire = serde_json::to_value(envelope()).expect("serializes");
    let keys: Vec<&String> = wire.as_object().expect("an object").keys().collect();
    assert_eq!(keys.len(), declared.len(), "no undeclared field: {keys:?}");

    for (field, kind) in &declared {
        let mut missing = wire.clone();
        missing.as_object_mut().expect("an object").remove(field);
        let read = serde_json::from_value::<EventEnvelope<Value>>(missing).is_ok();
        assert_eq!(
            read,
            kind.starts_with("Optional<"),
            "{field}: {kind} is read when missing iff it is Optional"
        );
        if kind == "Integer" {
            assert!(
                wire[field].is_i64(),
                "{field} is an Integer: {}",
                wire[field]
            );
            let mut fractional = wire.clone();
            fractional[field] = json!(1.5);
            assert!(
                serde_json::from_value::<EventEnvelope<Value>>(fractional).is_err(),
                "{field}: a fractional Integer is refused"
            );
        }
    }
}

#[test]
fn the_timebase_serializes_to_the_declared_variants_in_order() {
    let variants = type_entry("inference.stream.Timebase")
        .into_iter()
        .find_map(|line| line.trim_start().strip_prefix("variants: "))
        .expect("the timebase declares its variants");
    let declared: Vec<Value> = variants
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|variant| json!(variant.trim()))
        .collect();
    assert_eq!(
        serde_json::to_value(Timebase::ALL).expect("serializes"),
        Value::Array(declared)
    );
}
