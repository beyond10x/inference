---
format: aep.planning-md/3
id: story:subgraph-nested-drain-residual-loss
kind: story
status: draft
title: Text sent before stop is not lost two subgraphs deep under load
relations:
- decomposes: epic:session-facade
- serves: vision:inference
revision: 1
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
