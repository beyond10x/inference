---
format: aep.planning-md/3
id: story:usage-unconnected-port-counted
kind: story
status: implemented
title: Usage that reaches no edge is counted or marks the run total partial
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
scope:
- confidence: inferred
  path: crates/inference-core/src/usage.rs
- confidence: cited
  path: crates/inference-facade/src/live.rs
- confidence: cited
  path: crates/inference-facade/src/usage.rs
- confidence: inferred
  path: crates/inference-facade/tests
- confidence: cited
  path: crates/inference-runtime/src/controller.rs
- confidence: cited
  path: crates/inference-runtime/src/subgraph.rs
- confidence: inferred
  path: crates/inference-runtime/tests/adversary1_unconnected_usage_port.rs
- confidence: cited
  path: ess/domains/graph.yaml
- confidence: cited
  path: ess/domains/usage.yaml
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-21T09:08:56Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-21T09:08:56Z", actor: "human:timo", revision: 5}
- {from: "active", to: "implemented", at: "2026-10-21T11:54:16Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1}}}
---
## Context

Wave 26 (`story:subgraph-unconnected-usage-port`) made an unconnected output port of a subgraph node
read and drop the child's egress, matched to a model node whose port has no edge: that node gets no
endpoint and never sends its usage record. In both cases the run's usage total leaves the usage out
and nothing records that it did, so a session total can read complete while usage was spent.

## Planned scope

- Usage that reaches no edge, from a model node with no `usage` edge or a subgraph node whose `usage`
  port has none, is still counted in the run's usage total, or the total is marked partial with the
  node named; it is never left out silently.
- `ess/domains/usage.yaml` and `graph.yaml` state which of the two holds.

## Acceptance

A pipeline whose model node, and one whose subgraph node, has no edge on its `usage` port reports a
run usage total that either includes that node's usage or is marked partial naming the node.

- `task check` passes.
