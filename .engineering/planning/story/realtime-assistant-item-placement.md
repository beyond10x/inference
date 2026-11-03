---
format: aep.planning-md/3
id: story:realtime-assistant-item-placement
kind: story
status: implemented
title: Realtime client adds an assistant item, placed by previous_item_id, with its own event_id
summary: conversation.item.create for user, assistant and function-output items with optional previous_item_id and event_id.
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
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-03T11:15:59Z", actor: "human:timo", revision: 2, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
- {from: "proposed", to: "active", at: "2026-11-03T11:15:59Z", actor: "human:timo", revision: 3, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
- {from: "active", to: "implemented", at: "2026-11-03T18:16:39Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":2,"review_outcome":1}}}
---
## Context

A downstream adopter reported that the Realtime client can add only user text to the
conversation: `send_text` writes a `message` item with `role: "user"`
(`adapters/inference-openai/src/client.rs:192-206` on 0.9.0, verified). The adopter needs to add
an assistant message (a greeting it already played, a scripted line), placed after a given item
through `previous_item_id`, and to give the client event its own `event_id` so a server `error`
naming that `event_id` can be matched to the write that caused it.

## Planned scope

- `adapters/inference-openai/src/client.rs`: a conversation item type covering a user text
  message, an assistant text message and a function call output, and one method that sends
  `conversation.item.create` for an item with an optional `event_id` and an optional
  `previous_item_id`. `send_text` and `send_function_output` keep their wire shape and become
  callers of it.

## Acceptance

Named scenarios, offline against the local WebSocket test server of `tests/offline.rs`:

- `assistant_item_is_sent_with_role_assistant`: an assistant text item reaches the wire as a
  `message` item with `role: "assistant"` and one text content part.
- `item_placement_sends_previous_item_id`: a set `previous_item_id` is sent at the top level of
  the event; unset, the key is absent.
- `item_event_id_is_sent`: a set `event_id` is sent at the top level of the event; unset, the key
  is absent.
- `send_text_wire_shape_unchanged`: `send_text` still sends the 0.9.0 user message.

## Cause

Verified in code: `client.rs:197` on 0.9.0 hard-codes `"role": "user"`, and no send method takes
an `event_id` or `previous_item_id`.
