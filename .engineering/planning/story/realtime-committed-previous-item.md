---
format: aep.planning-md/3
id: story:realtime-committed-previous-item
kind: story
status: active
title: AudioCommitted keeps previous_item_id
summary: input_audio_buffer.committed is read with its previous_item_id.
tags:
- downstream-adopter
- openai-realtime
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-openai/src/event.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-03T11:16:00Z", actor: "human:timo", revision: 2, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
- {from: "proposed", to: "active", at: "2026-11-03T11:16:00Z", actor: "human:timo", revision: 3, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
---
## Context

A downstream adopter reported that `ServerEvent::AudioCommitted` drops the `previous_item_id` the
server sends with `input_audio_buffer.committed`, so a client cannot place the committed user
item in its own copy of the conversation (`adapters/inference-openai/src/event.rs:198-200` on
0.9.0, verified: only `item_id` is read).

## Planned scope

- `adapters/inference-openai/src/event.rs`: `AudioCommitted` gains
  `previous_item_id: Option<String>`, `None` when the server sent `null` or left it out.

## Acceptance

Named scenarios, parse tests in `tests/offline.rs` or the crate's event tests:

- `committed_keeps_previous_item_id`: a `committed` event with `previous_item_id` reads it.
- `committed_with_null_previous_item_id_is_none`: `null` and a missing key both read as `None`.

## Cause

Verified in code: `event.rs:71` declares `AudioCommitted { item_id }` and `event.rs:198-200`
reads only `item_id` on 0.9.0.
