---
format: aep.planning-md/3
id: story:llm-usage-on-cancel
kind: story
status: active
title: A cancelled or superseded generation still delivers its usage records
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-llama-server/tests
- confidence: cited
  path: crates/inference-llm
- confidence: cited
  path: crates/inference-testkit/src/llm
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-21T03:08:09Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-21T03:08:10Z", actor: "human:timo", revision: 4}
---
## Context

`inference-llm`'s `GenerationHandle::cancel()` and `supersede()` drop the usage records the node has
not yet taken from the generation (`crates/inference-llm/src/lib.rs`, the documentation of
`supersede`: "nothing more is delivered, the usage record of a turn not yet delivered among it").
A cancelled or superseded turn still consumed the tokens the provider counted, so dropping its
record loses billed usage, which `epic:usage-and-cost` rules out. Found by the review of
`story:usage-session-totals` (wave 24), which made the testkit LLM, STT and TTS nodes send a record
they already hold whatever the cancel; this is the record the generation holds and the node never
takes.

## Planned scope

- `inference-llm`: a cancelled or superseded generation still delivers the usage record of every
  turn whose usage `llm` reported, marked `final: false` when the turn did not end, through the same
  `GenerationEvent::Usage`; the documentation of `cancel` and `supersede` says so.
- The testkit LLM node takes and sends it on its `usage` port as it does now.
- `inference-llama-server`: a live test cancels a turn mid-output and finds its usage record.

## Acceptance

A generation on the mock LLM superseded during its second turn delivers two usage records, the
second with `final: false`, and a facade session counts both in its total, the second as partial.

- `task check` passes.
