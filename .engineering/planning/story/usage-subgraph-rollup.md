---
format: aep.planning-md/3
id: story:usage-subgraph-rollup
kind: story
status: implemented
title: Usage of a subgraph's child runs rolls up into the parent run's total
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-facade/src/usage.rs
- confidence: cited
  path: crates/inference-pipeline
- confidence: cited
  path: crates/inference-runtime
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-21T03:08:10Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-21T03:08:11Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-21T07:17:56Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1}}}
---
## Context

`decision-blocker:usage-subgraph-rollup` is cleared: roll up. A run total includes its child runs;
a subgraph node forwards the child run's usage on its own `usage` port, and each record keeps the
child run id, so the child total stays derivable. `story:usage-session-totals` (wave 24) counts
usage for pipelines without subgraph nodes only and says so in `crates/inference-facade/src/usage.rs`.

## Planned scope

- `inference-pipeline` / `inference-runtime`: a subgraph node exposes a `usage` output and forwards
  every usage envelope of its child run on it, the envelope's run id unchanged.
- `inference-facade`: the run total of a parent includes its child runs' records; the scope sentence
  in `src/usage.rs` goes.
- Tests: a pipeline with a subgraph node holding a model node counts that node's usage once in the
  parent run's total and keeps the child run id on the record.

## Acceptance

A session over a pipeline whose LLM sits inside a subgraph node delivers that LLM's usage record
once, with the child run id, and the parent run's total and the session total count it.

- `task check` passes.

## Closing note

The Planned scope's "the envelope's run id unchanged" was not followed: the envelope that crosses
to the parent takes the parent run id, as every envelope crossing a subgraph boundary does (A-14),
and the record's own `run_id` keeps naming the child run, as `ess/domains/usage.yaml` states (a
record names the child run; a parent run's total rolls its child runs up). The facade keys totals
by the envelope's run, so it counts the record once, under the parent run and in the session total.

Spec change: in `ess/domains/graph.yaml` a subgraph node's structured output port `usage` also
matches every child egress exposing a structured output port `usage` that no port of the node
names; a usage port forwarding nothing, or a usage egress with no port, is still
`SubgraphInterfaceMismatch`. 462 scenarios, 4 synthesis refusals, under ess 0.60.0 and 0.66.0.

The review found a parent cancel losing the child's cut-turn record (a mechanism older than this
story); fixed: at a cancel the egress bridge keeps reading usage egresses until the child run is
terminal, and their sends to the parent do not give way to the cancel. A new wait follows from it:
a full, unread `pause` queue on a parent usage edge can hold the subgraph node's stop during a
cancel; no test reaches it.

Two pre-existing defects the review measured are drafted as their own stories:
`story:subgraph-nested-ingress-drain` and `story:subgraph-unconnected-usage-port`.

