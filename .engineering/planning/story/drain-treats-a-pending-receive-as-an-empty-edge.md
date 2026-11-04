---
format: aep.planning-md/3
id: story:drain-treats-a-pending-receive-as-an-empty-edge
kind: story
status: active
title: A subgraph drain keeps an envelope an edge holds behind a pending receive
summary: The ingress bridge ends for the drain only on edges that are empty, not on a receive that answered Pending once.
tags:
- from-finding
relations:
- decomposes: epic:session-facade
- serves: vision:inference
scope:
- confidence: inferred
  path: crates/inference-core/src/endpoint.rs
- confidence: cited
  path: crates/inference-runtime/src/subgraph.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-04T07:29:54Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-04T07:29:54Z", actor: "human:operator", revision: 3}
---
## Context

Found by the adversary pass on story:subgraph-nested-drain-residual-loss in the 0.13.0 wave
(2026-11-04), `review-result:adversary-subgraph-nested-drain-pass-1`. The subgraph node's ingress
bridge (`crates/inference-runtime/src/subgraph.rs`, `ingress_bridge`) ends for the drain when
every edge's receive answered `Pending`, so it takes `Pending` for an empty edge.
`InputEndpoint::recv` promises "waiting while the edge is empty" and not that `Pending` means
empty. A receive that pends once while it holds an envelope makes the bridge send `stop`. The
envelope stays on the edge and is dropped with it, the drain answers `Ok` and the child
completes without it.

Reproduced with a test endpoint: the case
`text_on_an_edge_whose_receive_is_ready_only_on_a_later_poll_crosses` in
`crates/inference-runtime/tests/adversary1_drain_window_edges.rs`, commit `7498ad9` on branch
`impl/subgraph-nested-drain-residual-loss` (not merged), fails with `texts: [],
left_on_the_edge: 2`. The runtime's own receivers (`QueueReceiver`, `SinkReceiver`) pend only
when empty, so no edge inside the crate is known to reach it; an endpoint from outside the crate
can. Pre-existing: the bridge before 0.13.0 also took a pending receive for an empty edge (read
from the code, not run on the base).

## Planned scope

- Take the adversary case from `7498ad9` as the red test.
- Either drain only after a non-blocking check that each edge is empty, or state in the
  `InputEndpoint` contract that `Pending` from `recv` means the edge is empty and test the
  runtime's receivers against it. Pick one and record why.

## Acceptance

An envelope that an edge's receive holds when the node drains reaches the child, or the
`InputEndpoint` contract says such an endpoint is not admitted and a test holds the runtime's
receivers to it; the adversary case is green or replaced by that contract test.
