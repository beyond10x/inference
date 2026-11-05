---
format: aep.planning-md/3
id: story:run-ingress-admitted-send-outlives-the-drain
kind: story
status: active
title: A send a run ingress admitted before its drain is delivered or reported
summary: Measure whether a send admitted under the pause policy can still wait when the drain ends the bridge, and close the gap if it can.
tags:
- from-finding
relations:
- decomposes: epic:session-facade
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-runtime/src/drive.rs
- confidence: cited
  path: crates/inference-runtime/src/queue.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-04T07:29:54Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-04T07:29:54Z", actor: "human:operator", revision: 3}
---
## Context

Raised by the adversary pass on story:subgraph-nested-drain-residual-loss in the 0.13.0 wave
(2026-11-04), `review-result:adversary-subgraph-nested-drain-pass-1`, as undecided and read from
the code only. The 0.13.0 fix of the ingress bridge rests on "the parent drains the node only once
everything it admitted is on the node's edges". At a run's own ingress
(`crates/inference-runtime/src/drive.rs`, `GuardedSender`, about line 437) the admission
(`admits_input`) is checked on the first poll and the send then awaits room under the pause
policy, so a send admitted before the drain could still be waiting when the bridge finds the
edge empty and ends. That send would answer `Closed`, or its envelope be discarded with the
queue. Not reproduced.

## Planned scope

- Measure first: a deterministic test that fills a run ingress under the pause policy, admits a
  send, drains, and records whether the admitted envelope reaches the node or what the send
  returns.
- If it is lost silently: make admission and the push one step, or make the drain wait for every
  admitted send.

## Acceptance

A send a run ingress admitted before its drain either reaches the node or returns an error that
says it was not delivered; a test holds whichever the runtime does.
