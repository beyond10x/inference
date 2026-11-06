---
format: aep.planning-md/3
id: story:llm-wrapper-outcomes-and-debug
kind: story
status: active
title: The LLM wrapper keeps partial usage of a failed turn, redacts its Debug and tells truncation and refusal apart
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
- serves: vision:inference
scope:
- confidence: inferred
  path: crates/inference-llm/src/lib.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T00:41:05Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-06T00:41:05Z", actor: "human:timo", revision: 4}
---
## Context

From review-result:full-codebase-2026-11-06 (findings N25, N26, N27), at release 0.16.0, all in `crates/inference-llm/src/lib.rs`:

- N25 `:1290-1297` vs `:1346-1362`: `turn_failed` records usage as unknown even when the error's observation carries partial usage; `cut_turn_ended` keeps it. `ess/domains/usage.yaml:197` anticipates partial counts.
- N26 `:889-901`: `Debug for GenerationHandle` prints the conversation and user inputs that `AdmittedInput`'s Debug hides.
- N27 `:1225-1243`, `:1264-1273`: `StopReason::MaxOutputTokens`/`Incomplete` and `Item::AssistantRefusal` map to no `GenerationEvent`; a truncated or refused turn completes like a normal one (design 7.2).

## Planned scope

- A failed turn records the observation's partial usage with `final: false`, unknowns kept unknown.
- `Debug for GenerationHandle` redacts conversation and inputs as `AdmittedInput` does.
- The completion event carries the stop reason, and a refusal is delivered as a refusal; ESS first where the event gains a field.

## Acceptance

- A turn failing with partial usage records those counts, not unknown.
- `format!("{handle:?}")` contains no user text or instructions.
- A turn stopped by max output tokens completes with that stop reason; a refusal is delivered as a refusal with its text.
