---
format: aep.planning-md/3
id: story:realtime-live-into-split
kind: story
status: implemented
title: Realtime and GPT-Live clients split into a sender and a receiver
summary: An optional into_split so one task reads events while another writes.
tags:
- downstream-adopter
- openai-realtime
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-openai/src/client.rs
- confidence: inferred
  path: adapters/inference-openai/src/lib.rs
- confidence: cited
  path: adapters/inference-openai/src/live.rs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-11-03T15:55:02Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-03T15:55:02Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-11-03T18:16:39Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":1}}}
---
## Context

A downstream adopter asked for an optional `into_split` on `RealtimeClient` and `LiveClient`: a
sending half and a receiving half that two tasks can own, so one task reads events while another
writes audio. On 0.9.0 every method takes `&mut self` on one socket
(`adapters/inference-openai/src/client.rs`, `adapters/inference-openai/src/live.rs`).

On 0.10.0 the adopter's realtime voice session shows provider events waiting behind a stalled
caller-audio socket write: while `append_audio` awaits a write the peer does not drain, the same
`&mut RealtimeClient` cannot be polled for `next_event`, so every server event waits for the
write. The adopter bounded the write as a stopgap; the fix is two halves that two tasks own.

## Why it was not in the 0.10.0 wave

Not small: each client's whole send surface moves to a sender type, the receiver keeps the
pending queue, `unparsed_type` and the read timeout, and `close` needs both halves back, for two
clients. Filed as a draft by the 0.10.0 wave; accepted for 0.11.0.

## Planned scope

- `adapters/inference-openai/src/client.rs`: `RealtimeClient::into_split` returning a sender that
  owns every send method (`update_session`, `append_audio`, `append_encoded`, `commit_audio`,
  `clear_audio`, `ping`, `create_item`, `send_text`, `send_function_output`, `create_response`,
  `create_response_with`, `cancel_response`, `truncate`) and a receiver that owns `next_event`,
  `unparsed_type`, `session`, the events kept during setup and the call-name memory; a way to put
  the halves back together (or close from them) so the close handshake still runs.
- `adapters/inference-openai/src/live.rs`: the same for `LiveClient` (sender: `append_audio`,
  `mute`, `unmute`, the three appends, `send_function_output`, `create_response`, `ping`,
  `request_close`; receiver: `next_event`, `session`, `session_id`, the events kept during start),
  and a graceful close from the halves.
- `adapters/inference-openai/src/lib.rs`: re-exports.
- Out of scope: the duplex bindings (`src/duplex.rs`, `src/live_duplex.rs`) keep using the whole
  client.

## Acceptance

Named scenarios, offline against a local WebSocket test server, no network and no key:

- `realtime_split_reads_events_while_a_write_stalls`: the test server stops reading the socket,
  one task writes `append_encoded` chunks until a write stays pending, and while that write is
  still pending (its task not finished) the server sends server events, which the receiver half
  returns in order within the read timeout.
- `live_split_reads_events_while_a_write_stalls`: the same for `LiveClient`'s halves.
- `realtime_split_halves_send_and_read_as_the_client_did`: each send method of the sender writes
  the same client event as the whole client's method, and the receiver returns events kept during
  setup first, as `next_event` did.
- `realtime_split_halves_close_the_session`: the halves put back together (or closed from)
  complete the close handshake as `RealtimeClient::close` does; the same for `LiveClient`'s
  graceful close reading `session.closed`.

The whole client keeps its API; splitting is optional.
