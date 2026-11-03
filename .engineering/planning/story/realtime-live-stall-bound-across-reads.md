---
format: aep.planning-md/3
id: story:realtime-live-stall-bound-across-reads
kind: story
status: active
title: The Realtime and GPT-Live stall bound holds across reads
summary: The read deadline runs from the last message, not from each call, so a caller that drops and repeats reads still sees Stalled.
tags:
- downstream-adopter
- openai-realtime
relations:
- decomposes: epic:native-voice
- serves: vision:inference
- depends_on: story:realtime-live-into-split
scope:
- confidence: cited
  path: adapters/inference-openai/src/client.rs
- confidence: cited
  path: adapters/inference-openai/src/live.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-03T15:55:03Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-03T15:55:03Z", actor: "human:operator", revision: 3}
---
## Context

A downstream adopter reported, in its words, that `LiveClient` `read_text` "restarts its stall
deadline on every call, so a caller that loops on it never hits the stall bound". Verified in
code: `read_text` (`adapters/inference-openai/src/client.rs:492-531` on 0.10.0, shared by
`RealtimeClient::read_event` at `client.rs:433-446` and `LiveClient::read_event` at
`live.rs:973-976`) sets its deadline to `now + read_timeout` on every call. Inferred, not
observed: a caller that `select!`s `next_event` against its own audio source, or wraps it in a
shorter timeout, drops the read future and calls again, so the deadline moves forward with every
call and `Stalled` never fires while the server sends nothing. The adopter named the line as `client.rs:346`; on 0.10.0
`read_text` begins at `client.rs:492` (verified by reading the file at 0.10.0).

## Decision

The bound holds across calls. Each client (and each receiver half of
story:realtime-live-into-split) keeps the instant of the last message it read (a text or binary
message; control frames do not count, as today), starting at the end of the handshake, and every
read waits at most until that instant plus the endpoint's read timeout. `Stalled` then means "no
message for the read timeout", whatever number of calls or dropped reads that spans. A message
already buffered is still returned after the bound passed. The per-call reading is not kept as an
option: it is the defect.

## Planned scope

- `adapters/inference-openai/src/client.rs`: `read_text` takes the deadline (or the last-arrival
  instant) from its caller; `RealtimeClient` and its receiver half keep it.
- `adapters/inference-openai/src/live.rs`: `LiveClient` and its receiver half keep it;
  `close_seeing` keeps its own overall bound.
- The rustdoc of `next_event` on both clients states the bound.

## Acceptance

Named scenarios, offline against a local WebSocket test server, tokio's paused clock allowed:

- `live_stall_bound_holds_across_dropped_reads`: a caller that wraps `LiveClient::next_event` in a
  timeout shorter than the read timeout and calls again each time it expires, while the server
  sends nothing, gets `RealtimeError::Stalled` once the read timeout has passed since the last
  message, not later.
- `realtime_stall_bound_holds_across_dropped_reads`: the same for `RealtimeClient`.
- `stall_bound_restarts_on_each_message`: a server that sends a message every half read timeout
  never stalls the reader.
- `pings_do_not_restart_the_stall_bound`: pings and pongs alone do not move the bound, as before.
