---
format: aep.planning-md/3
id: story:subgraph-unconnected-usage-port
kind: story
status: implemented
title: An unconnected usage port of a subgraph node neither loses usage nor fails the run
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-pipeline
- confidence: cited
  path: crates/inference-runtime
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-21T07:28:54Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-21T07:28:54Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-21T08:36:21Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1}}}
---
## Context

The review of `story:usage-subgraph-rollup` (wave 25) measured that a subgraph node whose `usage`
port has no edge in the parent drops the child's usage egress end, so the child model's usage send
gets `Closed` and the parent run fails; a model node with no edge on its own `usage` port completes.
Two ignored cases in `crates/inference-runtime/tests/adversary1_subgraph_usage_rollup.rs` show it,
through the roll-up port and through a port named after the egress.

## Planned scope

- Decide by planning or by runtime: either validation refuses a subgraph node whose `usage` port
  forwards usage but has no edge (usage would be lost), or the runtime keeps reading the child's
  usage egress and records its envelopes as unconsumed, so the run completes and nothing is silent.
  Usage must never be lost silently and must never fail a run on its own.
- The two ignored cases run, adjusted to the chosen rule.

## Acceptance

A pipeline with a subgraph node whose `usage` port has no edge is either refused at planning with an
error naming the node and the port, or completes with the child's usage observable; it never fails
the run.

- `task check` passes.


## Closing note (wave 26)

The Planned scope offered two rules. The wave took the runtime rule, matched to a model node: a
subgraph node's output port with no edge in the parent gets a bridge that reads the child's egress
and drops what it reads, so the run completes. A model node's port with no edge gets no endpoint and
sends nothing, so in both cases the parent keeps no trace of that usage; a `Recorder` around the
child's nodes still sees the child's records, which is how the Acceptance's "child's usage observable"
is met. "Nothing is silent" in the Planned scope is not met by this story: counting usage that reaches
no edge, for model and subgraph nodes alike, is `story:usage-unconnected-port-counted`.

The rule covers every optional output port of a subgraph node, not only `usage`, because the same
defect failed the run for an unconnected text port. The drop-bridge ends when its egresses close or
when the child run has returned, so a child node that keeps an endpoint past `stop` does not hang
the parent's drain. `ess/domains/graph.yaml` and the facade's usage documentation now state that the
roll-up holds when the `usage` port has an edge.

