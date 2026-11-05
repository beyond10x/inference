---
format: aep.planning-md/3
id: story:refused-send-before-cancel-keeps-partial-usage
kind: story
status: implemented
title: A model node whose send is refused just before its cancel keeps the cut turn's usage
summary: A send refused while the run is cancelled ends the turn as cancelled with its partial usage, not as a failure.
tags:
- from-finding
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-testkit/src/llm/node.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-04T07:29:54Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-04T07:29:54Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-05T02:46:33Z", actor: "human:operator", revision: 5, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":1}}}
---
## Context

Found while implementing story:rollup-cancel-test-records-partial-usage in the 0.13.0 wave
(2026-11-04), traced with a diagnostic copy of the test, not committed. On the base the case
`a_cancel_of_the_parent_with_its_usage_port_connected_to_nothing_cancels_the_child`
(`crates/inference-runtime/tests/adversary1_subgraph_usage_rollup.rs`) failed 1 in 38,839 runs.
The model node had sent turn 2's `TextDelta` and was sending its `TextSegmentCommitted` when the
parent cancel ended the `reply` bridge; the send was refused ("the receiving side of the edge is
gone; event … was not sent"), the node failed instead of being cancelled, and a failed node skips
the code that sends the cut turn's usage record. The wave fixed the test (it waits for the commit
before cancelling); the node's behaviour is unchanged.

Read from the code, not reproduced through a real adapter: the testkit's model node
(`crates/inference-testkit/src/llm/node.rs`) fails on a refused send that lands just before its
cancel, and so records no partial usage for the cut turn. Whether an adapter-backed model node
reaches the same timing is not established.

## Planned scope

- Measure first: a deterministic test that refuses the model node's send just before its cancel
  arrives, and records which records the recorder holds.
- If the cut turn's usage is lost: a send refused because the run is being cancelled ends the turn
  as cancelled and sends its usage record, as a cancel does.

## Acceptance

A model node whose output send is refused while its run is being cancelled is recorded as
cancelled, not failed, and the cut turn's partial usage is in the run's usage records as non-final.
