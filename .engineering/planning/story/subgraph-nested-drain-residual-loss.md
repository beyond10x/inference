---
format: aep.planning-md/3
id: story:subgraph-nested-drain-residual-loss
kind: story
status: active
title: Text sent before stop is not lost two subgraphs deep under load
relations:
- decomposes: epic:session-facade
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-runtime/src/subgraph.rs
- confidence: cited
  path: crates/inference-runtime/tests/subgraph_drain_window.rs
- confidence: cited
  path: crates/inference-runtime/tests/subgraph_nested_drain.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-04T03:53:29Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-04T03:53:29Z", actor: "human:timo", revision: 3}
---
## Context

`crates/inference-runtime/tests/subgraph_nested_drain.rs` (the acceptance test of
story:subgraph-nested-ingress-drain, implemented) failed once during the wave-27 review of
story:duplex-late-usage-counted, in a full `cargo test` run on a tree that does not touch
`inference-runtime`:

```
2 levels: the text was lost or the run did not complete in 1 of 200 runs; first: [
    (
        16,
        "Ended { state: Completed, committed: [] }",
    ),
]
```

The run completed, but the text sent right before `stop` never reached the inner model: the loss
the fix in `drain_child` (OpenIngress guard) was meant to close. Its commit measured 0 losses in
4000 runs per depth; this is 1 in 200 under a loaded machine (the whole workspace suite running in
parallel). Every later run of the gate passed. Not reproduced on demand yet.

## Planned scope

- Measure first: run the two-level case in a loop under CPU load (for example the workspace suite
  in parallel, or `stress`-style busy threads from the test itself) until it fails or 20 000 runs
  pass; record the rate.
- If it reproduces: find the window. Candidates read from the code, unverified: an ingress bridge
  task that has not yet registered its OpenIngress guard when `drain_child` checks for open
  bridges; or the outer bridge's forward still in flight on the edge when the guard drops.
- Fix it in the runtime so the drain waits for every forward that was accepted before it, and
  keep the test's 200-run bound.

## Acceptance

- `subgraph_nested_drain` passes 20 000 runs per depth under the measured load with 0 losses.
- The measurement that found the window is recorded as evidence.

## Scope

Confirmed against the merged diff (0.13.0 wave, 245173c to 8b47266).

| path | planned | landed |
|---|---|---|
| `crates/inference-runtime/src/subgraph.rs` | cited | cited: `ingress_bridge` reads the drain before it polls its edges; not `drain_child` and not the OpenIngress guard, the story's two candidates |
| `crates/inference-runtime/tests/subgraph_nested_drain.rs` | cited | unchanged, RUNS stays 200; it is the acceptance loop |
| `crates/inference-runtime/tests/subgraph_drain_window.rs` | not planned | cited, new: the window forced without a sleep |

The adversary case `crates/inference-runtime/tests/adversary1_drain_window_edges.rs` is on the
unit branch at `7498ad9` and not merged; it belongs to
`story:drain-treats-a-pending-receive-as-an-empty-edge`.
