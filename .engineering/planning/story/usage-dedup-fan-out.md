---
format: aep.planning-md/3
id: story:usage-dedup-fan-out
kind: story
status: active
title: A usage record a fan-out hands to several outputs counts once in the run and session totals
summary: Totals count distinct usage_record_id; every output still delivers the record.
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-facade/src/live.rs
- confidence: cited
  path: crates/inference-facade/src/usage.rs
- confidence: cited
  path: crates/inference-facade/tests/usage.rs
- confidence: inferred
  path: crates/inference-runtime/src/usage_sink.rs
- confidence: cited
  path: ess/domains/usage.yaml
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-23T02:01:50Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, executor: "agent:implementor"}
- {from: "proposed", to: "active", at: "2026-11-23T02:01:50Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}, executor: "agent:implementor"}
---
## Context

A review of release 0.31.0 split one usage record across two outputs with a fan-out node and
read the session's usage view. The fan-out hands each envelope to every output edge unchanged
(`crates/inference-pipeline/src/nodes/fan_out.rs:115-128`), so the record reaches two egresses
with the same `usage_record_id`. The live session counts every usage record it reads off an
egress or a usage sink (`crates/inference-facade/src/live.rs:1276-1281`, `Shared::count_usage`)
and `SessionUsage::add` adds it to the run's total and the session's
(`crates/inference-facade/src/usage.rs:71-75`) with no regard to its identity. One request was
counted as two in both totals, and with the feature `cost` priced twice; both runs completed
normally. The single-branch control counted one.

The identity is the specification's: `inference.usage.UsageRecord` is identified by
`usage_record_id` (`ess/domains/usage.yaml:160-163`), one record per model call. A total is a sum
of records, so it counts each identity once.

## Scope

- `crates/inference-facade/src/usage.rs`: `SessionUsage` remembers the identities it counted and
  adds a record whose identity it counted already to neither total. cited
- `crates/inference-facade/src/live.rs`: `Shared::count_usage` prices only a record the usage
  view counted for the first time; delivery of every envelope stays as it is. cited
- `crates/inference-runtime/src/usage_sink.rs`: inferred to keep no total; it queues envelopes
  for the session to read. Checked, not changed. inferred
- `ess/domains/usage.yaml`: the entity's comment states that a total counts a record once by its
  identity, however many edges carry it. cited
- `crates/inference-facade/tests/usage.rs`: the cases below. cited

## Not covered

Another reader that sums `UsageRecorded` events it receives itself (an application's own meter)
is the application's; the facade's delivery of each copy is unchanged, so such a meter must key
on `usage_record_id` as the facade does. The chat recipe's usage wiring is another story.

## Acceptance

- A session over a pipeline whose model node's `usage` port feeds a fan-out node with two output
  edges, each exposed as a usage egress, answers one turn whose model reports one request: the
  session delivers the record once per egress (two `SessionEvent::UsageRecorded` with the same
  `usage_record_id`), and the run's total and the session's each hold one record, one request
  (`fan_out_of_one_usage_record_counts_it_once_and_delivers_it_on_each_output`).
- The same pipeline with one output edge on the fan-out delivers the record once and counts one
  record, one request (`fan_out_of_one_usage_record_onto_one_output_counts_it_once`).
- `cargo test -p inference-facade`, `cargo clippy -p inference-facade --all-targets
  --all-features -D warnings`, `cargo fmt --all --check` and `ess specify validate --path ess`
  pass.
