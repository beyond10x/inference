---
format: aep.planning-md/3
id: story:runtime-subgraph-and-recorder-bounds
kind: story
status: implemented
title: Runtime subgraphs end with their parent and never spin, and refusals and recordings are bounded
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
- serves: vision:inference
scope:
- confidence: inferred
  path: crates/inference-runtime/src/drive.rs
- confidence: inferred
  path: crates/inference-runtime/src/record.rs
- confidence: inferred
  path: crates/inference-runtime/src/subgraph.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T00:41:04Z", actor: "human:timo", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-06T00:41:04Z", actor: "human:timo", revision: 6}
- {from: "active", to: "implemented", at: "2026-11-06T13:31:04Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"test_result":1,"review_outcome":2}}}
---
## Context

From review-result:full-codebase-2026-11-06 (findings N1 to N5), at release 0.16.0, all in `crates/inference-runtime`:

- N1 `src/subgraph.rs:247-251`, `:399-403`: `SubgraphNode` has no `Drop`; a parent controller dropped mid-run leaves the child's thread parked forever.
- N2 `subgraph.rs:807-821`: a bridge send refused `Full` on a `fail` edge with no room handle yields by `wake_by_ref`, a busy-wait under the thread-parking `block_on`.
- N3 `subgraph.rs:776-782`, `:1301-1307`: a usage egress send never gives way to a cancel; a consumer holding its receiver unread hangs `stop` and `wind_down`.
- N4 `src/drive.rs:147`, `:267`, `:272`: `Signals.refusals` grows without a cap; `refusals()` clones it all.
- N5 `src/record.rs:17`, `:442-447`, `:515`: the recorder keeps every envelope in memory for the run's life.

## Planned scope

- `Drop` for `SubgraphNode` cancels the child run and joins its thread within a stated bound.
- The bridge waits on a room signal or a parked timer instead of re-polling at once.
- The usage egress send races the cancel.
- Cap `refusals` and count the overflow.
- The recorder takes a stated bound (entries or bytes) and counts what it dropped; the default bound is stated in the crate doc.

## Acceptance

- Dropping a parent controller mid-run ends the child thread within the stated bound.
- A slow parent consumer on a `fail` edge does not spin the child thread (poll count over a fixed window stays bounded).
- A cancel while a usage send waits ends `stop`.
- More refusals than the cap leave the list at the cap with the overflow counted.
- A run recording more than the recorder's bound keeps memory within it and reports the drop count.
