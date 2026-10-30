---
format: aep.planning-md/3
id: story:rollup-cancel-test-records-partial-usage
kind: story
status: draft
title: A parent cancel's rollup test sees the child's partial usage every run
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-runtime/tests/adversary1_subgraph_usage_rollup.rs
revision: 2
---
## Context

`inference / task check` on the 0.9.0 release pull request (beyond10x/inference#17, run
41109795491, 2026-10-30) failed one test in a crate the wave did not touch:

```
---- a_cancel_of_the_parent_with_its_usage_port_connected_to_nothing_cancels_the_child stdout ----
panicked at crates/inference-runtime/tests/adversary1_subgraph_usage_rollup.rs:1289:9:
assertion `left == right` failed
  left: [true]
 right: [true, false]
test result: FAILED. 11 passed; 1 failed
```

The wave changed nothing under `crates/inference-runtime` or `crates/inference-pipeline`
(`git diff --stat origin/main..wave/0.9.0` lists neither), and `Cargo.lock` moved only the ess crates
and the workspace versions. The test sends a second turn, waits for one text event on `reply`, and
cancels; the assertion expects the child's partial usage of that turn to be recorded as non-final.
Read from the code, not reproduced: the cancel can land before the child's model records the
partial usage, so only the first turn's final usage is there. 0 of 1 local runs measured.

## Planned scope

- Measure first: run the test in a loop (at most 4 workers, under `nice`) until it fails or 2000
  runs pass; record the rate.
- If it reproduces: make the test wait for the partial usage it asserts (or for the event that
  precedes it) before cancelling, or, if the runtime drops a partial usage it should keep, fix the
  runtime.

## Acceptance

`cargo test -p inference-runtime --test adversary1_subgraph_usage_rollup` passes 2000 runs in a row
with the assertion `finals == [true, false]` unchanged.
