//! Adversary pass 1 against `inference-tools`: the confirmation flag a declaration carries, which
//! no case of the unit reads back with the value `false`, and the order the confirmation hook takes
//! against the state check for a call that is not Proposed, which the unit documents and no case
//! of its own exercises.

use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    AuthorizeError, AuthorizeToolCall, ConfirmationRequired, DeclareTool, RejectionReason,
    SideEffectClass, Tool, ToolCall, ToolCallState, ToolCallStateConflict,
};
use serde_json::json;

fn declare(requires_confirmation: bool) -> Tool {
    Tool::declare(DeclareTool {
        name: "transfer".to_owned(),
        schema: json!({"type": "object"}),
        schema_revision: "7".to_owned(),
        side_effect: SideEffectClass::NonIdempotentWrite,
        requires_confirmation,
    })
    .0
}

fn committed_span() -> Span {
    let (turn, _) = Turn::open(OpenTurn {
        run_id: RunId::generate(),
    });
    Span::emit(
        &turn,
        turn.run_id(),
        StreamId::generate(),
        1,
        "send it".to_owned(),
    )
    .expect("an Open turn admits a span")
    .0
    .commit()
    .expect("a Hypothesis commits")
    .0
}

fn proposed(tool: &Tool) -> ToolCall {
    let generation = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0;
    ToolCall::propose(tool, &generation, &committed_span(), json!({}))
        .expect("a committed span admits the proposal")
        .0
}

fn input(call: &ToolCall, confirmation_required: bool, confirmed: bool) -> AuthorizeToolCall {
    AuthorizeToolCall {
        call_id: call.call_id(),
        confirmation_required,
        confirmed,
    }
}

/// `requires_confirmation` is the one field of a tool the authority reads for the confirmation
/// hook; every case of the unit declares it `true` or never reads it back.
#[test]
fn a_declaration_keeps_requires_confirmation_as_declared() {
    assert!(!declare(false).requires_confirmation());
    assert!(declare(true).requires_confirmation());
    let tool = declare(false);
    assert_eq!(tool.name(), "transfer");
    assert_eq!(tool.schema_revision(), "7");
    assert_eq!(tool.schema(), &json!({"type": "object"}));
}

/// `ToolCall::authorize` documents the hook before the state check, as the specification orders
/// `unconfirmed` before `wrong-state`; the unit exercises the hook on a Proposed call only, where
/// either order answers the same.
#[test]
fn the_hook_answers_before_the_state_for_a_call_that_is_not_proposed() {
    let call = proposed(&declare(true));
    let authorized = call
        .authorize(&input(&call, true, true))
        .expect("Proposed authorizes with a confirmation")
        .0;
    let rejected = call
        .reject(RejectionReason::Denied)
        .expect("Proposed rejects")
        .0;
    for held in [&authorized, &rejected] {
        assert_eq!(
            held.authorize(&input(held, true, false)),
            Err(AuthorizeError::Unconfirmed(ConfirmationRequired {
                call_id: held.call_id(),
            })),
            "{:?}",
            held.state()
        );
        assert_eq!(
            held.authorize(&input(held, true, true)),
            Err(AuthorizeError::Conflict(ToolCallStateConflict {
                current: held.state(),
            })),
            "{:?}",
            held.state()
        );
    }
    assert_eq!(authorized.state(), ToolCallState::Authorized);
}
