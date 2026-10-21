---
format: aep.planning-md/3
id: story:usage-subgraph-rollup
kind: story
status: active
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
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-21T03:08:10Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-21T03:08:11Z", actor: "human:timo", revision: 4}
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
