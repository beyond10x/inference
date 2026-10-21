---
format: aep.planning-md/3
id: story:drain-admits-no-new-input
kind: story
status: draft
title: Input sent after a drain was requested never reaches a model
relations:
- decomposes: epic:session-facade
- serves: vision:inference
revision: 1
---
## Context

The review of `story:subgraph-nested-ingress-drain` (wave 26) read, without running it, that during a
drain an ingress bridge keeps forwarding while its edge has envelopes ready, and the parent run's
ingress still takes sends while the run is Draining (`crates/inference-runtime/src/drive.rs`, the
ingress send path; `crates/inference-runtime/src/subgraph.rs`, `ingress_bridge`).
`ess/domains/graph.yaml` says a drain admits no new input. Inferred, not measured.

## Planned scope

- Measure first: a caller asks for a drain without `stop` and keeps sending; count envelopes that
  reach a model after the drain started. If none do in 1000 runs, record that and close the story.
- Otherwise the run's ingress refuses a send once the run is Draining, and a bridge forwards only
  what it was handed before the drain.

## Acceptance

Input sent after a drain was requested never reaches a model, in a flat run and through two subgraph
levels, measured over at least 200 runs each.

- `task check` passes.
