---
format: aep.planning-md/3
id: story:nova-sonic-stall-and-bounds
kind: story
status: active
title: Nova Sonic reports one stall per idle limit and bounds its arrivals and open
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
- serves: vision:inference
scope:
- confidence: inferred
  path: adapters/inference-aws/src/client.rs
- confidence: inferred
  path: adapters/inference-aws/src/duplex.rs
- confidence: inferred
  path: adapters/inference-aws/src/nova_sonic/session.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T00:41:02Z", actor: "human:operator", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-06T00:41:02Z", actor: "human:operator", revision: 6}
---
## Context

From review-result:full-codebase-2026-11-06 (findings W3, N9, N10, N11), at release 0.16.0:

- W3 `adapters/inference-aws/src/nova_sonic/session.rs:247-259`, `duplex.rs:417-435`: after one `Stall` every later `recv()` returns `Stall` at once, because `activity` is touched only on a delivered frame; `forward` loops on `Received::Quiet`, spinning a pool worker and growing the unbounded `arrivals` channel while the session is quiet with nothing awaited. Contradicts `session.rs:243-246`.
- N9 `duplex.rs:356-359`: the SDK's bounded `frames` channel is drained into an unbounded `std_mpsc`.
- N10 `duplex.rs:345-354`, `client.rs:86-93`: `open` has no deadline of the binding's own on the stream's `send()`.
- N11 `duplex.rs:875-878`: substituting the provider's `sampleRateHertz` into output frames can only do nothing or make every frame a counted ledger drop.

## Planned scope

- A stall restarts the idle deadline, so the next `recv()` waits a full `idle_limit` again.
- Bound the `arrivals` channel; the reader waits while it is full.
- One stated deadline over `open`.
- A provider rate other than the configured one fails the session with a typed error instead of substituting it.

## Acceptance

- A quiet session past `idle_limit` reports one `Stall` per `idle_limit`, not a stream of them (counted over a fixed window).
- A caller that stops calling `next_event` holds the channel at its bound.
- A server that never sends response headers makes `open` fail within the stated deadline.
- An output configuration at another rate ends the session with the typed error.
