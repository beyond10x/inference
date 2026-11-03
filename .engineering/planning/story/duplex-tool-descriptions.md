---
format: aep.planning-md/3
id: story:duplex-tool-descriptions
kind: story
status: implemented
title: Duplex bindings send each tool's description
summary: Tool gains a specified description that both OpenAI duplex bindings send.
tags:
- downstream-adopter
- openai-realtime
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-openai/src/duplex.rs
- confidence: cited
  path: adapters/inference-openai/src/live_duplex.rs
- confidence: cited
  path: crates/inference-tools/src/lib.rs
- confidence: cited
  path: ess/domains/tools.yaml
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-03T15:55:02Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-03T15:55:03Z", actor: "human:operator", revision: 4}
- {from: "active", to: "implemented", at: "2026-11-03T18:16:39Z", actor: "human:operator", revision: 9, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":1}}}
---
## Context

A downstream adopter reported that the duplex bindings send every tool to the provider with an
empty description (`adapters/inference-openai/src/duplex.rs:344` and
`adapters/inference-openai/src/live_duplex.rs:350` on 0.9.0, verified: `description:
String::new()`; on 0.10.0 the same literal is at `duplex.rs:348` and `live_duplex.rs:350`). A
model chooses a tool by its description, so the duplex path cannot be used for tools. The cause
is upstream of the bindings: `inference_tools::Tool` has no description
(`crates/inference-tools/src/lib.rs:395-402`), and neither does `inference.tools.Tool` or
`inference.tools.DeclareTool` in `ess/domains/tools.yaml`.

## Why it was not in the 0.10.0 wave

Not small: a description is a new field on a specified entity and command (spec first), on the
public `DeclareTool` input that many files construct as a struct literal, and then on both
bindings. Filed as a draft by the 0.10.0 wave; accepted for 0.11.0.

## Planned scope

- `ess/domains/tools.yaml`: `description` on `inference.tools.Tool` and `DeclareTool`, with a
  `read from:` line, validated by `ess specify validate --path ess`.
- `crates/inference-tools`: the field and its accessor; every `DeclareTool` literal in the
  workspace.
- `adapters/inference-openai/src/duplex.rs`, `adapters/inference-openai/src/live_duplex.rs`: send
  the tool's description in the provider's function tool.
- Any other binding that offers tools to a provider and drops the description
  (`adapters/inference-aws` is checked, and changed only if it has a description field to fill).

## Acceptance

Named scenarios:

- `declare_tool_keeps_its_description` (`crates/inference-tools`): a tool declared with a
  description returns it from its accessor; a description left empty is the empty string, as
  declared.
- `realtime_duplex_sends_each_tool_description` (`adapters/inference-openai`, offline WebSocket
  server): a duplex session opened with two tools sends `session.update` whose `tools[*].description`
  is each tool's declared description.
- `live_duplex_sends_each_tool_description` (`adapters/inference-openai`, offline, feature
  `gpt-live`): the GPT-Live duplex binding's `session.start` carries each tool's declared
  description in its Responses backend `tools`.
- `ess specify validate --path ess` passes with `description` on `inference.tools.Tool` and
  `inference.tools.DeclareTool`.
