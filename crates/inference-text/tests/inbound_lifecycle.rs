//! `inference.stream.Turn` and `inference.stream.Span` follow the lifecycles of
//! `ess/domains/stream.yaml`: a turn opens Open and completes once; a span is emitted as a
//! Hypothesis into an Open turn only, is revised while it is a Hypothesis, and is committed once.
//! A revision of a committed span is refused and leaves the text as it was, the first contract
//! test of the design (`docs/design/inference.md:1105`).

use inference_core::__private::uuid::Uuid;
use inference_core::id::{RunId, StreamId};
use inference_text::{
    CommittedSpanImmutable, OpenTurn, Span, SpanId, SpanNotFound, SpanState, SpanStateConflict,
    TranscriptCommitted, TranscriptHypothesis, TranscriptRevision, Turn, TurnCompleted, TurnId,
    TurnNotFound, TurnOpened, TurnState, TurnStateConflict,
};
use serde_json::json;

fn run_id() -> RunId {
    RunId::from(Uuid::from_u128(0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d))
}

fn stream_id() -> StreamId {
    StreamId::from(Uuid::from_u128(0x2a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d))
}

fn open() -> Turn {
    Turn::open(OpenTurn { run_id: run_id() }).0
}

fn hypothesis(turn: &Turn) -> Span {
    Span::emit(turn, run_id(), stream_id(), 3, "hello wor".to_owned())
        .expect("an Open turn admits a span")
        .0
}

#[test]
fn a_turn_opens_open_and_completes_once() {
    let turn_id = TurnId::from(Uuid::from_u128(0x11));
    let (turn, opened) = Turn::open_as(turn_id, OpenTurn { run_id: run_id() });
    assert_eq!(turn.turn_id(), turn_id);
    assert_eq!(turn.run_id(), run_id());
    assert_eq!(turn.state(), TurnState::Open);
    assert_eq!(
        opened,
        TurnOpened {
            turn_id,
            run_id: run_id()
        }
    );

    let (completed, event) = turn.complete().expect("an Open turn completes");
    assert_eq!(completed.state(), TurnState::Completed);
    assert_eq!(event, TurnCompleted { turn_id });
    assert_eq!(
        completed.complete(),
        Err(TurnStateConflict {
            current: TurnState::Completed
        })
    );
    assert_eq!(turn.state(), TurnState::Open, "a move returns a new turn");
}

#[test]
fn a_span_is_a_hypothesis_of_an_open_turn_and_a_completed_turn_admits_none() {
    let turn = open();
    let span_id = SpanId::from(Uuid::from_u128(0x22));
    let (span, event) = Span::emit_as(
        span_id,
        &turn,
        run_id(),
        stream_id(),
        3,
        "hello wor".to_owned(),
    )
    .expect("an Open turn admits a span");
    assert_eq!(span.span_id(), span_id);
    assert_eq!(span.run_id(), run_id());
    assert_eq!(span.turn_id(), turn.turn_id());
    assert_eq!(span.stream_id(), stream_id());
    assert_eq!(span.sequence(), 3);
    assert_eq!(span.text(), "hello wor");
    assert_eq!(span.state(), SpanState::Hypothesis);
    assert_eq!(
        event,
        TranscriptHypothesis {
            span_id,
            turn_id: turn.turn_id(),
            sequence: 3,
            text: "hello wor".to_owned(),
        }
    );

    let (completed, _) = turn.complete().expect("completes");
    assert_eq!(
        Span::emit(&completed, run_id(), stream_id(), 4, "late".to_owned()),
        Err(TurnStateConflict {
            current: TurnState::Completed
        })
    );
}

#[test]
fn a_hypothesis_is_revised_and_a_committed_span_refuses_the_revision_unchanged() {
    let span = hypothesis(&open());
    let (revised, event) = span
        .revise("hello world".to_owned())
        .expect("a Hypothesis is revised");
    assert_eq!(revised.text(), "hello world");
    assert_eq!(revised.state(), SpanState::Hypothesis);
    assert_eq!(
        event,
        TranscriptRevision {
            span_id: span.span_id(),
            text: "hello world".to_owned(),
        }
    );

    let (committed, _) = revised.commit().expect("a Hypothesis commits");
    assert_eq!(
        committed.revise("goodbye".to_owned()),
        Err(CommittedSpanImmutable {
            span_id: span.span_id()
        })
    );
    assert_eq!(committed.text(), "hello world", "the text is unchanged");
    assert_eq!(committed.state(), SpanState::Committed);
}

#[test]
fn a_hypothesis_commits_once_with_its_text() {
    let span = hypothesis(&open());
    let (committed, event) = span.commit().expect("a Hypothesis commits");
    assert_eq!(committed.state(), SpanState::Committed);
    assert_eq!(
        event,
        TranscriptCommitted {
            span_id: span.span_id(),
            text: "hello wor".to_owned(),
        }
    );
    assert_eq!(
        committed.commit(),
        Err(SpanStateConflict {
            current: SpanState::Committed
        })
    );
}

#[test]
fn a_committed_span_outlives_the_completion_of_its_turn() {
    let turn = open();
    let (committed, _) = hypothesis(&turn).commit().expect("commits");
    let (completed, _) = turn.complete().expect("completes");
    assert_eq!(completed.state(), TurnState::Completed);
    assert_eq!(committed.state(), SpanState::Committed);
    assert_eq!(committed.turn_id(), completed.turn_id());
}

#[test]
fn states_events_and_errors_are_written_under_their_ess_names() {
    assert_eq!(
        serde_json::to_value(TurnState::ALL).expect("serializes"),
        json!(["Open", "Completed"])
    );
    assert_eq!(
        serde_json::to_value(SpanState::ALL).expect("serializes"),
        json!(["Hypothesis", "Committed"])
    );

    let turn = open();
    let span = hypothesis(&turn);
    let turn_id = turn.turn_id().to_string();
    let span_id = span.span_id().to_string();
    assert_eq!(
        serde_json::to_value(&span).expect("serializes"),
        json!({
            "span_id": span_id,
            "run_id": run_id().to_string(),
            "turn_id": turn_id,
            "stream_id": stream_id().to_string(),
            "sequence": 3,
            "text": "hello wor",
            "state": "Hypothesis",
        })
    );
    assert_eq!(
        serde_json::to_value(&turn).expect("serializes"),
        json!({"turn_id": turn_id, "run_id": run_id().to_string(), "state": "Open"})
    );
    assert_eq!(
        serde_json::to_value(TurnNotFound {
            turn_id: turn.turn_id()
        })
        .expect("serializes"),
        json!({"turn_id": turn_id})
    );
    assert_eq!(
        serde_json::to_value(SpanNotFound {
            span_id: span.span_id()
        })
        .expect("serializes"),
        json!({"span_id": span_id})
    );
    assert_eq!(
        serde_json::to_value(CommittedSpanImmutable {
            span_id: span.span_id()
        })
        .expect("serializes"),
        json!({"span_id": span_id})
    );
    assert_eq!(
        serde_json::to_value(TurnStateConflict {
            current: TurnState::Completed
        })
        .expect("serializes"),
        json!({"current": "Completed"})
    );
    assert_eq!(
        serde_json::to_value(SpanStateConflict {
            current: SpanState::Committed
        })
        .expect("serializes"),
        json!({"current": "Committed"})
    );
}
