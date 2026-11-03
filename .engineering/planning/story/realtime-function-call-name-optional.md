---
format: aep.planning-md/3
id: story:realtime-function-call-name-optional
kind: story
status: implemented
title: A function call without a name is read, its name carried from the output item
summary: FunctionCall.name is optional and filled from the matching function_call output item when the server sent one.
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
  path: adapters/inference-openai/src/duplex.rs
- confidence: cited
  path: adapters/inference-openai/src/event.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-03T11:16:00Z", actor: "human:operator", revision: 2, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
- {from: "proposed", to: "active", at: "2026-11-03T11:16:00Z", actor: "human:operator", revision: 3, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
- {from: "active", to: "implemented", at: "2026-11-03T18:16:40Z", actor: "human:operator", revision: 7, decided_on: {"recorded":{"test_result":2,"review_outcome":1}}}
---
## Context

A downstream adopter reported that `response.function_call_arguments.done` is refused as a
protocol error when it carries no `name` (`adapters/inference-openai/src/event.rs:206-212` on
0.9.0, verified: `name: event.string("name")?`). The adopter has not checked whether today's
server omits it. The repository holds no copy of the provider's event schema: its only witnesses
are hand-written test events that carry `name` (`tests/offline.rs:646`, `tests/duplex.rs:716`).
So the field is read as optional, and the function name is carried from the matching
`response.output_item.added` or `response.output_item.done` event, whose `item` of type
`function_call` names it, when the server sent one.

## Planned scope

- `adapters/inference-openai/src/event.rs`: `FunctionCall.name` becomes `Option<String>`; a
  missing or `null` `name` is no longer a protocol error.
- `adapters/inference-openai/src/client.rs`, the read path only (`read_event` and what it
  calls): the client remembers the name of each `function_call` output item by item id and fills
  a `FunctionCall` that arrives without one. The memory is bounded.
- `adapters/inference-openai/src/duplex.rs`: the binding reads the optional name; a call that has
  no name after the client's fill takes the binding's existing path for a tool not offered.

## Acceptance

Named scenarios, offline:

- `function_call_without_name_parses`: the done event without `name` reads with `name: None`.
- `function_call_name_carried_from_output_item`: an `output_item.added` naming the function,
  then a done event without `name` for the same item, gives `FunctionCall` with that name from
  `next_event`.
- `function_call_with_name_unchanged`: a done event with `name` reads it as on 0.9.0.
- `duplex_call_without_any_name_is_not_offered`: through the duplex binding, a call with no name
  anywhere is reported as the binding reports a tool not offered, and the session continues.

## Cause

Verified in code: `event.rs:210` on 0.9.0 requires `name`. Whether the server omits it is not
verified (no provider schema in the repository).
