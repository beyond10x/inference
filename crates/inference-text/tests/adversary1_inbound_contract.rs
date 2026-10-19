//! Adversary cases for the inbound text of `inference-text`, driven from `ess/domains/stream.yaml`
//! read as written: the inputs of `OpenTurn`, `EmitHypothesis` and `ReviseSpan`, the five inbound
//! events and the four inbound errors serialize exactly the declared fields in the declared order;
//! and a revision moves the text only.

use inference_core::__private::uuid::Uuid;
use inference_core::id::{RunId, StreamId};
use inference_text::{
    CommittedSpanImmutable, EmitHypothesis, OpenTurn, ReviseSpan, Span, SpanId, SpanNotFound,
    SpanState, SpanStateConflict, TranscriptCommitted, TranscriptHypothesis, TranscriptRevision,
    Turn, TurnCompleted, TurnId, TurnNotFound, TurnOpened, TurnState, TurnStateConflict,
};
use serde::Serialize;

const STREAM_YAML: &str = include_str!("../../../ess/domains/stream.yaml");

/// The field names (`fields:` or `input:`) of the entry `name` of the top-level `section`, in the
/// order they are written.
fn declared(section: &str, name: &str) -> Vec<String> {
    let mut inside = false;
    let mut entry = false;
    let mut key = String::new();
    let mut fields = Vec::new();
    for line in STREAM_YAML.lines() {
        let body = line.trim_start();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        match line.len() - body.len() {
            0 => inside = body == format!("{section}:"),
            2 if inside => {
                entry = body == format!("- name: {name}");
                key.clear();
            }
            4 if inside && entry => body.trim_end_matches(':').clone_into(&mut key),
            6 if inside && entry && (key == "fields" || key == "input") => {
                if let Some(field) = body.strip_prefix("- name: ") {
                    fields.push(field.to_owned());
                }
            }
            _ => {}
        }
    }
    assert!(!fields.is_empty(), "{section} {name} declares fields");
    fields
}

/// The keys `value` serializes, in the order they are written.
fn written<T: Serialize>(value: &T) -> Vec<String> {
    let text = serde_json::to_string(value).expect("serializes");
    let object = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text)
        .expect("an object");
    let mut keys: Vec<(usize, String)> = object
        .keys()
        .map(|key| {
            let at = text.find(&format!("\"{key}\":")).expect("written");
            (at, key.clone())
        })
        .collect();
    keys.sort();
    keys.into_iter().map(|(_, key)| key).collect()
}

fn id(low: u128) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_4000_8000_0000_0000_0000 | low)
}

#[test]
fn inbound_inputs_events_and_errors_write_the_declared_fields_in_order() {
    let run_id = RunId::from(id(1));
    let turn_id = TurnId::from(id(2));
    let span_id = SpanId::from(id(3));
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "commands",
            "inference.stream.OpenTurn",
            written(&OpenTurn { run_id }),
        ),
        (
            "commands",
            "inference.stream.EmitHypothesis",
            written(&EmitHypothesis {
                run_id,
                turn_id,
                stream_id: StreamId::from(id(4)),
                sequence: 1,
                text: "a".to_owned(),
            }),
        ),
        (
            "commands",
            "inference.stream.ReviseSpan",
            written(&ReviseSpan {
                span_id,
                text: "a".to_owned(),
            }),
        ),
        (
            "events",
            "inference.stream.TurnOpened",
            written(&TurnOpened { turn_id, run_id }),
        ),
        (
            "events",
            "inference.stream.TurnCompleted",
            written(&TurnCompleted { turn_id }),
        ),
        (
            "events",
            "inference.stream.TranscriptHypothesis",
            written(&TranscriptHypothesis {
                span_id,
                turn_id,
                sequence: 1,
                text: "a".to_owned(),
            }),
        ),
        (
            "events",
            "inference.stream.TranscriptRevision",
            written(&TranscriptRevision {
                span_id,
                text: "a".to_owned(),
            }),
        ),
        (
            "events",
            "inference.stream.TranscriptCommitted",
            written(&TranscriptCommitted {
                span_id,
                text: "a".to_owned(),
            }),
        ),
        (
            "errors",
            "inference.stream.TurnStateConflict",
            written(&TurnStateConflict {
                current: TurnState::Open,
            }),
        ),
        (
            "errors",
            "inference.stream.TurnNotFound",
            written(&TurnNotFound { turn_id }),
        ),
        (
            "errors",
            "inference.stream.CommittedSpanImmutable",
            written(&CommittedSpanImmutable { span_id }),
        ),
        (
            "errors",
            "inference.stream.SpanNotFound",
            written(&SpanNotFound { span_id }),
        ),
        (
            "errors",
            "inference.stream.SpanStateConflict",
            written(&SpanStateConflict {
                current: SpanState::Hypothesis,
            }),
        ),
    ];
    for (section, name, keys) in cases {
        assert_eq!(keys, declared(section, name), "{section} {name}");
    }
}

#[test]
fn a_revision_moves_the_text_only() {
    let (turn, _) = Turn::open_as(
        TurnId::from(id(2)),
        OpenTurn {
            run_id: RunId::from(id(1)),
        },
    );
    let (span, _) = Span::emit_as(
        SpanId::from(id(3)),
        &turn,
        RunId::from(id(1)),
        StreamId::from(id(4)),
        9,
        "hello wor".to_owned(),
    )
    .expect("an Open turn admits a span");
    let (revised, _) = span.revise("hello world".to_owned()).expect("a Hypothesis");
    assert_eq!(
        (
            revised.span_id(),
            revised.run_id(),
            revised.turn_id(),
            revised.stream_id(),
            revised.sequence(),
            revised.state(),
        ),
        (
            span.span_id(),
            span.run_id(),
            span.turn_id(),
            span.stream_id(),
            span.sequence(),
            SpanState::Hypothesis,
        )
    );
    assert_eq!(revised.text(), "hello world");
}
