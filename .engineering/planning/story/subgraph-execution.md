---
format: aep.planning-md/3
id: story:subgraph-execution
kind: story
status: active
title: A subgraph node runs its child pipeline with cancellation and tracing boundaries kept (A-14)
summary: A runtime node implementation drives a nested child run from its instance's lifecycle; cancel and failure cross the boundary and every envelope stays attributable to the run that produced it.
relations:
- decomposes: epic:session-facade
- serves: vision:inference
- depends_on: story:subgraph-declaration
scope:
- confidence: cited
  path: crates/inference-runtime/src/controller.rs
- confidence: cited
  path: crates/inference-runtime/src/lib.rs
- confidence: cited
  path: crates/inference-runtime/src/subgraph.rs
- confidence: cited
  path: crates/inference-runtime/tests/subgraph.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:56:32Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T22:56:32Z", actor: "human:timo", revision: 4}
---
## Context

A-14 (`docs/design/inference.md:1150`): reusable subgraphs compose without losing cancellation and
tracing boundaries. The runtime owns the lifecycle of runs and node instances, cancellation,
failure propagation and trace propagation (655-662); traces SHOULD correlate session, graph run,
node execution, generation, stream events, tool call and sink acknowledgement (1032); a failed node
MUST surface a typed, actionable error (1006). `story:subgraph-declaration` declares the subgraph
node and a run's parent; this story executes it with `RunController`
(`crates/inference-runtime/src/controller.rs`) and the exposures' outer ends
(`story:pipeline-exposed-ports`). No trace facility exists in the tree today: an envelope carries
`run_id`, `event_id` and `causation_id` (`crates/inference-core/src/envelope.rs:126-141`), and a node
is given its run id when it is built (`crates/inference-pipeline/src/nodes/vad.rs:251`).

Decisions taken on silence:

- Execution is nested, not flattened: the subgraph node's implementation creates a child run of the
  child's planned pipeline with its own instance as parent, and drives it from its own lifecycle.
  Prepare creates, validates and prepares the child run; start activates it and bridges each of the
  node's edge endpoints to the child exposure of the same name; drain sends `stop` into the child's
  ingresses and waits for Completed; cancel cancels the child run; stop returns once the child run
  is terminal and its queues are dropped. Nesting is chosen because the run boundary then is the
  cancellation and tracing boundary A-14 asks to keep.
- Cancellation: cancelling the parent run cancels the child run, and every child instance is
  Stopped and every child queue dropped before the parent's subgraph instance stops. A child
  failure fails the subgraph instance, and with it the parent run, with a reason naming the
  subgraph node and carrying the child's reason.
- Tracing, with no tracing library: the boundary is the run. Every envelope inside the child carries
  the child's run id; an envelope crossing the boundary is emitted on the other side with that
  side's run id, a new event id, the subgraph node as producer and the crossed envelope's event id
  as `causation_id`. A run's parent is readable from its controller. Exporting traces is later
  telemetry work.
- Generation epochs inside a child are the child's; the parent does not supersede them.

## Planned scope

- `crates/inference-runtime/src/subgraph.rs` (new): the subgraph node implementation;
  `src/lib.rs` exports it; `src/controller.rs` creates a run with a parent and reads it back.
- Test `crates/inference-runtime/tests/subgraph.rs` (new): the A-02 text pipeline with exposures as
  the child inside a parent text pipeline, on the testkit mocks.

## Domain relations

- Node references Pipeline as its subgraph, zero or one, and Run references NodeInstance as its
  parent, zero or one: declared by `story:subgraph-declaration`.
- Run owns NodeInstance, many: `ess/domains/graph.yaml`, `inference.graph.Run`, relation
  `instances`.
- Generation references Run, one: `ess/domains/stream.yaml`, `inference.stream.Generation`,
  relation `run`; a child's generations reference the child run.

No noun is added and `ess/` does not change.

## Acceptance

`crates/inference-runtime/tests/subgraph.rs` runs the parent pipeline to Completed with the child's
reply reaching the parent's sink, every envelope logged inside the child carrying the child run id
and every crossed envelope carrying the parent run id and a `causation_id` naming its child-side
counterpart; cancelling the parent while the child streams leaves the child run Cancelled, every
child instance Stopped and every child queue dropped before the parent run is Cancelled; and a
child node that fails fails the parent run with a reason naming the subgraph node and the child
node.

## Out of scope

Flattened subgraphs; a voice frontend as a subgraph; trace export or a tracing library; a subgraph
inside a session; superseding a child's generations from the parent.
