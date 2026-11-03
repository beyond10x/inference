---
format: aep.planning-md/3
id: story:duplex-tool-descriptions
kind: story
status: draft
title: Duplex bindings send each tool's description
summary: Tool gains a specified description that both OpenAI duplex bindings send.
tags:
- downstream-adopter
- openai-realtime
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 1
---
## Context

A downstream adopter reported that the duplex bindings send every tool to the provider with an
empty description (`adapters/inference-openai/src/duplex.rs:344` and
`adapters/inference-openai/src/live_duplex.rs:350` on 0.9.0, verified: `description:
String::new()`). The cause is upstream of the bindings: `inference_tools::Tool` has no
description (`crates/inference-tools/src/lib.rs:395-402`), and neither does
`inference.tools.Tool` or `inference.tools.DeclareTool` in `ess/domains/tools.yaml`.

## Why it is not in the 0.10.0 wave

Not small: a description is a new field on a specified entity and command (spec first), on the
public `DeclareTool` input that 38 files construct as a struct literal, and then on both bindings.

## Planned scope

- `ess/domains/tools.yaml`: `description` on `inference.tools.Tool` and `DeclareTool`.
- `crates/inference-tools`: the field and its accessor; every `DeclareTool` literal.
- `adapters/inference-openai/src/duplex.rs`, `live_duplex.rs`: send the tool's description.

## Acceptance

To be written when the story is accepted.
