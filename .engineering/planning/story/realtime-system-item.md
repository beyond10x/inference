---
format: aep.planning-md/3
id: story:realtime-system-item
kind: story
status: implemented
title: An out-of-band Realtime response can carry a system-role item
summary: ConversationItem::SystemText, a message item with role system, for create_item and response input.
tags:
- downstream-adopter
- openai-realtime
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-openai/src/client.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-03T15:55:03Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-03T15:55:03Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-03T18:16:39Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

A downstream adopter asked that an out-of-band response (`response.create` with
`conversation: "none"`) can carry a system-role item in its `input`, the shape the Realtime guide
uses to give one response instructions in context without adding them to the conversation. On
0.10.0 `ConversationItem` has `UserText`, `AssistantText` and `FunctionCallOutput` only
(`adapters/inference-openai/src/client.rs:561-596`, verified), so neither
`ResponseOptions::input` nor `create_item` can send a `message` item with `role: "system"`.

## Planned scope

- `adapters/inference-openai/src/client.rs`: `ConversationItem::SystemText(String)`, a `message`
  item with `role: "system"` and one content part of type `input_text`, accepted wherever a
  `ConversationItem` is (`create_item`, `ResponseOptions::input`). Implemented in the same unit as
  story:realtime-live-stall-bound-across-reads (same file).

## Acceptance

Named scenarios, offline against the local WebSocket test server:

- `out_of_band_response_carries_a_system_item`: `create_response_with` with conversation none and
  `input` holding a `SystemText` sends `response.input[0]` as
  `{"type": "message", "role": "system", "content": [{"type": "input_text", "text": ...}]}`.
- `create_item_sends_a_system_item`: `create_item` with a `SystemText` sends the same item under
  `conversation.item.create`'s `item`.
