---
format: aep.planning-md/3
id: story:subgraph-unconnected-usage-port
kind: story
status: draft
title: An unconnected usage port of a subgraph node neither loses usage nor fails the run
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
revision: 1
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
