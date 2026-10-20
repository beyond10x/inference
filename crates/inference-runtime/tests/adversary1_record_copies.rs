//! Guarding case for the recorder (`story:control-event-replay`): an envelope a node passes on
//! unchanged, as a fan-out or a commit gate does, carries an event identity already recorded and
//! is the same decision, counted once, for the node that made it.

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_pipeline::PortId;
use inference_runtime::record::{DecisionKind, Entry, Recorded, Recording};

#[test]
fn a_copy_passed_on_unchanged_is_not_a_second_decision() {
    let (llm, split, gate) = (NodeId::generate(), NodeId::generate(), NodeId::generate());
    let envelope = EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: llm,
        port: "answer".to_owned(),
        kind: "inference.stream.TextSegmentCommitted".to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: (),
    };
    let entries = [llm, split, split, gate]
        .into_iter()
        .enumerate()
        .map(|(order, node_id)| Recorded {
            merge_order: order as u64,
            entry: Entry::Emitted {
                node_id,
                port_id: PortId::generate(),
                envelope: envelope.clone(),
            },
        })
        .collect();
    let recording = Recording {
        policy_version: "1".to_owned(),
        entries,
    };
    let decisions = recording.decisions();
    assert_eq!(
        decisions
            .iter()
            .map(|decision| (decision.node_id, decision.kind, decision.merge_order))
            .collect::<Vec<_>>(),
        [(llm, DecisionKind::Commit, 0)]
    );
}
