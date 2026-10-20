//! Adversary case for the replay comparison (`story:control-event-replay`): a control decision
//! is compared by node, port, kind and event name only, so two runs whose tool authority
//! authorized different tool calls compare as the same decisions.
//!
//! The recordings are built by hand from what a tool authority emits: one
//! `inference.tools.ToolCallAuthorized` each, from the same node on the same port, one naming a
//! read-only lookup and the other a side-effecting delete. The policy version differs as well.

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_pipeline::PortId;
use inference_runtime::record::{Entry, Recorded, Recording};
use inference_runtime::replay::first_divergence;
use serde_json::{Value, json};

fn authorized(node_id: NodeId, port_id: PortId, call: Value) -> Recording<Value> {
    let envelope = EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: node_id,
        port: "decisions".to_owned(),
        kind: "inference.tools.ToolCallAuthorized".to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: call,
    };
    Recording {
        policy_version: String::new(),
        entries: vec![Recorded {
            merge_order: 0,
            entry: Entry::Emitted {
                node_id,
                port_id,
                envelope,
            },
        }],
    }
}

#[test]
fn authorizing_a_different_tool_call_is_a_diverging_decision() {
    let (authority, port) = (NodeId::generate(), PortId::generate());
    let mut recorded = authorized(
        authority,
        port,
        json!({"tool": "weather.lookup", "side_effect": "read_only"}),
    );
    recorded.policy_version = "admits-read-only".to_owned();
    let mut replayed = authorized(
        authority,
        port,
        json!({"tool": "files.delete", "side_effect": "destructive"}),
    );
    replayed.policy_version = "admits-everything".to_owned();

    let (was, is) = (recorded.decisions(), replayed.decisions());
    assert_eq!((was.len(), is.len()), (1, 1), "one authorization each");
    assert!(
        first_divergence(&was, &is).is_some(),
        "the recording authorized weather.lookup and the replay files.delete, and the replay \
         reports no divergence: {was:?} / {is:?}"
    );
}
