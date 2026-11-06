---
format: aep.planning-md/3
id: story:live-session-close-and-bounded-log
kind: story
status: active
title: A draining live session closes when its run fails or is cancelled, and its event log is bounded
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
- serves: vision:inference
scope:
- confidence: inferred
  path: crates/inference-facade/src/live.rs
- confidence: inferred
  path: crates/inference-facade/src/session.rs
- confidence: inferred
  path: ess/domains/session.yaml
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T00:41:02Z", actor: "human:timo", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-06T00:41:02Z", actor: "human:timo", revision: 6}
---
## Context

From review-result:full-codebase-2026-11-06 (findings W4, W8, N24, N28), at release 0.16.0:

- W4 `crates/inference-facade/src/live.rs:401` with `:426-439`, `:331-345`: after `close()`, a run that ends Failed or Cancelled never closes the session; `receive()` answers `None` with the session still Draining and no `SessionClosed`. Contradicts `live.rs:26-27` and `ess/domains/session.yaml:452-465`.
- W8 `session.rs:674` from `live.rs:851-854`: `Sessions.events` is append-only, one `InputSent` per send (one per audio frame), Closed sessions kept; `events()` clones it all.
- N24 `live.rs:176-180`, `:946-947`, `:1080`, `:1085`: `Generations.cut`, `.delivered`, `.segments` are never pruned after a generation ends.
- N28 `live.rs:851-868`: `InputSent` is recorded before the ingress accepts; a refused send still yields `InputSent` (`session.yaml:500`).

## Planned scope

- A Draining session whose run ends Failed or Cancelled closes, delivering `RunFailed`/`RunCancelled` then `SessionClosed` last; `session.yaml` states it first.
- Bound the event log: per-frame inputs are counted, not each logged, and a Closed session's history is released after its last receive; `events()` returns a bounded view. State the bound in the crate doc and `session.yaml`.
- Prune generation bookkeeping when a generation ends.
- Record `InputSent` only after the ingress accepts.

## Acceptance

- `close()` then a node failure delivers `SessionClosed` and leaves the session Closed.
- `close()` then a cancel by a second holder delivers `SessionClosed`.
- Ten thousand audio sends leave the event log within its stated bound.
- Generation bookkeeping is empty after a generation ends.
- A refused ingress send yields no `InputSent`.
