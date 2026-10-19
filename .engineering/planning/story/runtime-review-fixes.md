---
format: aep.planning-md/3
id: story:runtime-review-fixes
kind: story
status: draft
title: 'Bounded edges: the four defects of the 0.3.0 review'
relations:
- decomposes: epic:graph-minimum
- serves: vision:inference
- depends_on: story:bounded-edges
scope:
- confidence: cited
  path: crates/inference-core/src/endpoint.rs
- confidence: cited
  path: crates/inference-runtime/src/lib.rs
- confidence: cited
  path: crates/inference-runtime/src/queue.rs
- confidence: cited
  path: crates/inference-runtime/tests
revision: 5
---
## Context

A read-only review of the changes since 0.3.0 (`review-result:review-030-runtime`, 2026-10-19) found four
defects in `crates/inference-runtime/src/queue.rs` and the endpoint docs, each traced to a line:

1. `QueueReceiver::drop` clears the queued envelopes without counting them, while the module doc
   and `discarded()` promise that every discarded envelope is counted (`queue.rs:413-422`, `:24`,
   `:348`).
2. `coalesce` discards the arriving envelope when no queued envelope shares its stream
   (`queue.rs:213-225`), keeping old envelopes over new ones. Design line 684 reads "coalesce or
   supersede old hypotheses".
3. Under `drop_stale_epochs` a pending send of a now-stale epoch is woken only when a purge freed an
   item of its producer (`queue.rs:186-204`), so it waits for the consumer although it is already
   stale.
4. `crates/inference-runtime/src/lib.rs:2-5` and `crates/inference-core/src/endpoint.rs:9-10` say the
   runtime builds queues per run; nothing does yet (`story:node-instance-lifecycle` will).

## Decisions

- Coalesce: when no queued envelope shares the arriving envelope's stream, the oldest queued
  envelope is discarded (and counted) and the arrival is queued; per-stream order is kept.
- The receiver's drop counts every envelope it clears.
- A newer epoch from a producer wakes that producer's pending sends of older epochs, which then
  complete as discarded.
- The two docs say what exists today and name the story that wires queues per run.

## Acceptance

`cargo test -p inference-runtime --all-features` passes with one case per decision above, each red
before the fix: dropped receiver counts its queued envelopes; coalesce with a full queue of other
streams keeps the arrival and discards the oldest; a stale pending send completes discarded without
a consumer pop.

## Out of scope

Wiring queues per run (`story:node-instance-lifecycle`).
