---
format: aep.planning-md/3
id: story:realtime-response-options
kind: story
status: active
title: Realtime response.create takes per-response options
summary: create_response with instructions, tool_choice, conversation none, input items, tools and event_id.
tags:
- downstream-adopter
- openai-realtime
relations:
- decomposes: epic:native-voice
- serves: vision:inference
- depends_on: story:realtime-session-input-options
- depends_on: story:realtime-assistant-item-placement
scope:
- confidence: cited
  path: adapters/inference-openai/src/client.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-03T11:15:59Z", actor: "human:timo", revision: 2, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
- {from: "proposed", to: "active", at: "2026-11-03T11:15:59Z", actor: "human:timo", revision: 3, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
---
## Context

A downstream adopter building a realtime voice backend on 0.9.0 reported that
`RealtimeClient::create_response` sends a bare `response.create`
(`adapters/inference-openai/src/client.rs:224-226` on 0.9.0, verified: the body is
`json!({"type": "response.create"})`). The adopter needs per-response options: instructions for
this response only, a tool choice, `conversation: "none"` for an out-of-band response that is not
added to the default conversation, explicit `input` items, and a tool list for this response.

## Planned scope

- `adapters/inference-openai/src/client.rs`: a `ResponseOptions` value (instructions, tool
  choice, conversation `auto` or `none`, input items, tools, event id) and a method that sends
  `response.create` with a `response` object holding only the options that are set.
  `create_response()` keeps sending the bare event.
- It reuses `ToolChoice` from story:realtime-session-input-options and the item type from
  story:realtime-assistant-item-placement; it depends on both.

## Acceptance

Named scenarios, offline against the local WebSocket test server of `tests/offline.rs`:

- `create_response_with_options_sends_each_set_field`: every option set reaches the wire under
  `response` with the reference's field names (`instructions`, `tool_choice`, `conversation`,
  `input`, `tools`), and `event_id` at the top level.
- `create_response_with_default_options_is_the_bare_event`: no option set sends exactly
  `{"type": "response.create"}`.
- `out_of_band_response_sends_conversation_none`: conversation none is the string `"none"`.

## Cause

Verified in code: `client.rs:224-226` on 0.9.0 takes no argument and writes no `response` object.
