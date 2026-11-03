---
format: aep.planning-md/3
id: story:realtime-live-keepalive-ping
kind: story
status: implemented
title: Realtime and GPT-Live clients send a keepalive ping
summary: A ping method on RealtimeClient and LiveClient writes one WebSocket ping frame.
tags:
- downstream-adopter
- openai-realtime
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-openai/src/client.rs
- confidence: cited
  path: adapters/inference-openai/src/live.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-03T11:16:00Z", actor: "human:timo", revision: 2, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
- {from: "proposed", to: "active", at: "2026-11-03T11:16:00Z", actor: "human:timo", revision: 3, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
- {from: "active", to: "implemented", at: "2026-11-03T18:16:40Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":2,"review_outcome":1}}}
---
## Context

A downstream adopter asked for a keepalive: a way to send a WebSocket ping on an idle Realtime or
GPT-Live session. On 0.9.0 neither `RealtimeClient` nor `LiveClient` can send one; the read path
skips pings and pongs the server sends (`adapters/inference-openai/src/client.rs:372`, verified).

## Planned scope

- `adapters/inference-openai/src/client.rs` and `adapters/inference-openai/src/live.rs`: a
  `ping` method on each client that writes one WebSocket ping frame with a caller-given payload of
  at most 125 bytes. The caller schedules it; no timer is added. A pong does not count as an
  event and does not reset the stall timer.

## Acceptance

Named scenarios, offline against the local WebSocket test servers:

- `realtime_ping_sends_a_ping_frame`: the server receives a ping frame with the payload.
- `live_ping_sends_a_ping_frame`: the same for `LiveClient` (feature `gpt-live`).
- `ping_payload_over_125_bytes_is_refused`: a longer payload is refused before any write.

## Cause

Verified in code: no `Message::Ping` is written anywhere in `adapters/inference-openai/src` on
0.9.0.
