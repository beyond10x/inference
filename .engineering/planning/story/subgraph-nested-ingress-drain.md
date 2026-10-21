---
format: aep.planning-md/3
id: story:subgraph-nested-ingress-drain
kind: story
status: implemented
title: Input sent before stop is never overtaken by the drain at a nested subgraph's ingress
relations:
- decomposes: epic:session-facade
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-runtime
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-21T07:28:54Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-21T07:28:54Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-21T08:36:21Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
## Context

The review of `story:usage-subgraph-rollup` (wave 25) measured input lost two subgraphs deep: text
and `stop` sent back to back through two subgraph levels lose the text in 6 of 160 runs; the reply
carries only `GenerationStarted`, `GenerationCompleted` and `stop`. Inferred mechanism, not run at
the base: the drain reaches the inner `ingress_bridge` (`crates/inference-runtime/src/subgraph.rs`,
the `Merged::Drain` arm) before the outer bridge has forwarded the text. `ingress_bridge` predates
wave 25.

## Planned scope

- `inference-runtime`: an ingress bridge forwards every envelope it was handed before it acts on a
  drain, at every nesting level, so input sent before `stop` is never overtaken.
- A test sends text and `stop` back to back through two and three subgraph levels and asserts the
  text arrives, repeated at least 200 times without a loss.

## Acceptance

Through two subgraph levels, text sent immediately before `stop` reaches the inner model in 200 of
200 runs, and the run completes.

- `task check` passes.
