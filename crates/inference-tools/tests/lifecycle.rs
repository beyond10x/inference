//! The tool-call lifecycle of `ess/domains/tools.yaml`, through the public API of
//! `inference-tools`: the two proposal paths and their barrier (§23.1 test 5 of the design), the
//! confirmation hook, every transition and every state a command refuses.

use inference_core::generation::{
    Generation, GenerationState, GenerationStateConflict, StartGeneration,
};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    AuthorizeError, AuthorizeToolCall, CancellationSignal, ConfirmationRequired, DeclareTool,
    ProposeError, ProposeSpeculativeError, ProvisionalInputBarrier, RejectionReason,
    SideEffectClass, Tool, ToolCall, ToolCallId, ToolCallState, ToolCallStateConflict, ToolFailure,
    ToolFailureKind,
};
use serde_json::json;

fn tool(side_effect: SideEffectClass, requires_confirmation: bool) -> Tool {
    Tool::declare(DeclareTool {
        name: "book".to_owned(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect,
        requires_confirmation,
    })
    .0
}

fn generation() -> Generation {
    Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0
}

fn hypothesis() -> Span {
    let (turn, _) = Turn::open(OpenTurn {
        run_id: RunId::generate(),
    });
    Span::emit(
        &turn,
        turn.run_id(),
        StreamId::generate(),
        1,
        "book it".to_owned(),
    )
    .expect("an Open turn admits a span")
    .0
}

fn committed() -> Span {
    hypothesis().commit().expect("a Hypothesis commits").0
}

fn proposed(tool: &Tool) -> ToolCall {
    ToolCall::propose(tool, &generation(), &committed(), json!({"day": "monday"}))
        .expect("a proposal on a committed span")
        .0
}

fn authorize(
    call: &ToolCall,
) -> Result<(ToolCall, inference_tools::ToolCallAuthorized), AuthorizeError> {
    call.authorize(&AuthorizeToolCall {
        call_id: call.call_id(),
        confirmation_required: false,
        confirmed: false,
    })
}

fn in_state(state: ToolCallState) -> ToolCall {
    let call = proposed(&tool(SideEffectClass::ReadOnly, false));
    let authorized = || authorize(&call).expect("Proposed authorizes").0;
    let started = || authorized().start().expect("Authorized starts").0;
    match state {
        ToolCallState::Proposed => call,
        ToolCallState::Authorized => authorized(),
        ToolCallState::Started => started(),
        ToolCallState::Completed => {
            started()
                .complete(json!({"ok": true}))
                .expect("Started completes")
                .0
        }
        ToolCallState::Failed => {
            started()
                .fail(ToolFailure {
                    kind: ToolFailureKind::ExecutionError,
                    detail: "down".to_owned(),
                })
                .expect("Started fails")
                .0
        }
        ToolCallState::Rejected => {
            call.reject(RejectionReason::Denied)
                .expect("Proposed rejects")
                .0
        }
    }
}

/// §23.1 test 5: uncommitted text cannot cause a side-effecting tool by default.
#[test]
fn an_idempotent_write_on_a_hypothesis_is_barred_and_on_a_committed_span_is_proposed() {
    let write = tool(SideEffectClass::IdempotentWrite, false);
    let generation = generation();

    let barred = ToolCall::propose(&write, &generation, &hypothesis(), json!({}));
    assert_eq!(
        barred,
        Err(ProposeError::Barred(ProvisionalInputBarrier {
            side_effect: SideEffectClass::IdempotentWrite,
        }))
    );

    let (call, event) = ToolCall::propose(&write, &generation, &committed(), json!({"a": 1}))
        .expect("a committed span admits the proposal");
    assert_eq!(call.state(), ToolCallState::Proposed);
    assert_eq!(call.side_effect(), SideEffectClass::IdempotentWrite);
    assert_eq!(call.arguments(), &json!({"a": 1}));
    assert_eq!(event.call_id, call.call_id());
    assert_eq!(event.tool_id, write.tool_id());
    assert_eq!(event.generation_id, generation.generation_id());

    let speculative = ToolCall::propose_speculative(&write, &generation, &committed(), json!({}));
    assert_eq!(
        speculative,
        Err(ProposeSpeculativeError::SideEffecting(
            ProvisionalInputBarrier {
                side_effect: SideEffectClass::IdempotentWrite,
            }
        ))
    );
}

#[test]
fn the_default_path_bars_a_hypothesis_for_every_class_and_the_speculative_path_admits_read_only() {
    for class in SideEffectClass::ALL {
        let refused =
            ToolCall::propose(&tool(class, false), &generation(), &hypothesis(), json!({}));
        assert_eq!(
            refused,
            Err(ProposeError::Barred(ProvisionalInputBarrier {
                side_effect: class
            })),
            "{class:?} on a hypothesis"
        );
    }
    let (call, _) = ToolCall::propose_speculative(
        &tool(SideEffectClass::ReadOnly, false),
        &generation(),
        &hypothesis(),
        json!({}),
    )
    .expect("a read-only tool may act on a hypothesis");
    assert_eq!(call.side_effect(), SideEffectClass::ReadOnly);
    for class in [
        SideEffectClass::IdempotentWrite,
        SideEffectClass::NonIdempotentWrite,
    ] {
        let refused = ToolCall::propose_speculative(
            &tool(class, false),
            &generation(),
            &hypothesis(),
            json!({}),
        );
        assert_eq!(
            refused,
            Err(ProposeSpeculativeError::SideEffecting(
                ProvisionalInputBarrier { side_effect: class }
            ))
        );
    }
}

#[test]
fn a_generation_that_is_not_active_proposes_nothing_before_the_barrier_is_read() {
    for (stale, state) in [
        (
            generation().complete().expect("completes").0,
            GenerationState::Completed,
        ),
        (
            generation().supersede().expect("supersedes").0,
            GenerationState::Superseded,
        ),
        (
            generation().cancel().expect("cancels").0,
            GenerationState::Cancelled,
        ),
    ] {
        let conflict = GenerationStateConflict { current: state };
        let write = tool(SideEffectClass::NonIdempotentWrite, false);
        assert_eq!(
            ToolCall::propose(&write, &stale, &hypothesis(), json!({})),
            Err(ProposeError::StaleGeneration(conflict))
        );
        assert_eq!(
            ToolCall::propose_speculative(&write, &stale, &hypothesis(), json!({})),
            Err(ProposeSpeculativeError::StaleGeneration(conflict))
        );
    }
}

#[test]
fn authorization_refuses_without_a_required_confirmation_and_the_call_stays_proposed() {
    let call = proposed(&tool(SideEffectClass::NonIdempotentWrite, true));
    let input = |confirmation_required, confirmed| AuthorizeToolCall {
        call_id: call.call_id(),
        confirmation_required,
        confirmed,
    };
    assert_eq!(
        call.authorize(&input(true, false)),
        Err(AuthorizeError::Unconfirmed(ConfirmationRequired {
            call_id: call.call_id(),
        }))
    );
    assert_eq!(call.state(), ToolCallState::Proposed);
    for (required, confirmed) in [(true, true), (false, false), (false, true)] {
        let (moved, event) = call
            .authorize(&input(required, confirmed))
            .unwrap_or_else(|error| panic!("required {required}, confirmed {confirmed}: {error}"));
        assert_eq!(moved.state(), ToolCallState::Authorized);
        assert_eq!(event.call_id, call.call_id());
    }

    let unknown = AuthorizeToolCall {
        call_id: ToolCallId::generate(),
        confirmation_required: true,
        confirmed: false,
    };
    assert_eq!(
        unknown.confirmation(),
        Err(ConfirmationRequired {
            call_id: unknown.call_id,
        }),
        "the hook reads the input alone, before any call is looked up"
    );
}

#[test]
fn each_transition_moves_from_its_source_states_only() {
    type Move = fn(&ToolCall) -> Result<ToolCallState, ToolCallStateConflict>;
    let moves: [(&str, Move, &[ToolCallState], ToolCallState); 5] = [
        (
            "authorize",
            |call| {
                authorize(call)
                    .map(|(moved, _)| moved.state())
                    .map_err(|error| match error {
                        AuthorizeError::Conflict(conflict) => conflict,
                        AuthorizeError::Unconfirmed(_) => panic!("no confirmation is required"),
                    })
            },
            &[ToolCallState::Proposed],
            ToolCallState::Authorized,
        ),
        (
            "reject",
            |call| {
                call.reject(RejectionReason::Superseded)
                    .map(|(moved, _)| moved.state())
            },
            &[ToolCallState::Proposed, ToolCallState::Authorized],
            ToolCallState::Rejected,
        ),
        (
            "start",
            |call| call.start().map(|(moved, _)| moved.state()),
            &[ToolCallState::Authorized],
            ToolCallState::Started,
        ),
        (
            "complete",
            |call| call.complete(json!(1)).map(|(moved, _)| moved.state()),
            &[ToolCallState::Started],
            ToolCallState::Completed,
        ),
        (
            "fail",
            |call| {
                call.fail(ToolFailure {
                    kind: ToolFailureKind::DeadlineExceeded,
                    detail: "late".to_owned(),
                })
                .map(|(moved, _)| moved.state())
            },
            &[ToolCallState::Started],
            ToolCallState::Failed,
        ),
    ];
    for (name, apply, from, to) in moves {
        for state in ToolCallState::ALL {
            let call = in_state(state);
            let answer = apply(&call);
            if from.contains(&state) {
                assert_eq!(answer, Ok(to), "{name} from {state:?}");
            } else {
                assert_eq!(
                    answer,
                    Err(ToolCallStateConflict { current: state }),
                    "{name} from {state:?}"
                );
                assert_eq!(call.state(), state, "a refused {name} moves nothing");
            }
        }
    }
}

#[test]
fn completed_failed_and_rejected_are_terminal() {
    for state in ToolCallState::ALL {
        assert_eq!(
            state.is_terminal(),
            matches!(
                state,
                ToolCallState::Completed | ToolCallState::Failed | ToolCallState::Rejected
            ),
            "{state:?}"
        );
    }
}

#[test]
fn events_carry_the_reason_the_result_and_the_failure() {
    let call = in_state(ToolCallState::Authorized);
    let (_, rejected) = call
        .reject(RejectionReason::Exhausted)
        .expect("Authorized rejects");
    assert_eq!(rejected.reason, RejectionReason::Exhausted);
    assert_eq!(rejected.call_id, call.call_id());

    let started = in_state(ToolCallState::Started);
    let (_, result) = started.complete(json!({"seat": 4})).expect("completes");
    assert_eq!(result.result, json!({"seat": 4}));
    let failure = ToolFailure {
        kind: ToolFailureKind::Cancelled,
        detail: "barge-in".to_owned(),
    };
    let (_, failed) = started.fail(failure.clone()).expect("fails");
    assert_eq!(failed.failure, failure);
}

#[test]
fn serialized_names_are_the_ess_names() {
    fn names<T: serde::Serialize>(values: &[T]) -> Vec<String> {
        values
            .iter()
            .map(|value| match serde_json::to_value(value) {
                Ok(serde_json::Value::String(name)) => name,
                other => panic!("not one string: {other:?}"),
            })
            .collect()
    }
    assert_eq!(
        names(&SideEffectClass::ALL),
        ["read_only", "idempotent_write", "non_idempotent_write"]
    );
    assert_eq!(
        names(&RejectionReason::ALL),
        ["schema_invalid", "denied", "superseded", "exhausted"]
    );
    assert_eq!(
        names(&ToolFailureKind::ALL),
        ["deadline_exceeded", "cancelled", "execution_error"]
    );
    assert_eq!(
        names(&ToolCallState::ALL),
        [
            "Proposed",
            "Authorized",
            "Started",
            "Completed",
            "Failed",
            "Rejected"
        ]
    );
}

#[test]
fn a_cancellation_signal_is_shared_by_its_clones() {
    let signal = CancellationSignal::new();
    let observed = signal.clone();
    assert!(!observed.is_cancelled());
    signal.cancel();
    assert!(observed.is_cancelled());
}
