---
format: aep.planning-md/3
id: epic:usage-and-cost
kind: epic
status: draft
title: Usage and cost events for every model call
relations:
- serves: vision:inference
revision: 1
---
## Context

The owner, 2026-10-20: "we do need to somehow emit usage/pricing events (tokens, duration, inputs,
outputs, etc)". Today usage is partial and scattered: `inference.duplex` records `ResponseUsage`
and `TranscriptionUsage` for native speech models (`ess/domains/duplex.yaml:94-115`);
`inference-llm` receives `llm`'s usage observation in each `TurnOutcome` (`llm_core::Usage`,
`llm` `crates/llm-core/src/turn.rs:271`) but emits no event; STT and TTS adapters report nothing;
local adapters measure nothing per call. The design lists "optional cost attributes" per adapter
(`docs/design/inference.md:439`) and quotas for P7 (line 1058) but no usage vocabulary.

The `llm` repository has `llm-cost`: priced charges from explicit, versioned price documents
(`PriceBook`, `CostReport`), unknown quantities kept unknown, no provider price lookup and no
currency conversion (`llm` `crates/llm-cost/src/lib.rs:1-8`).

## Outcome

Every model call in a run (STT, LLM, TTS, native speech; hosted and local) emits one usage record
on the run's reliable event path: who (run, session, node, adapter, model, provider), when
(started, ended, wall duration), and what (quantities by kind, each reported by the provider,
measured locally, or unknown). A caller that supplies a price document gets a cost record per usage
record and per session, with unknown prices and quantities kept unknown. Nothing is dropped:
usage never rides the best-effort telemetry channel.

## Scope

- A usage noun in `ess/` first (guardrail 7): the record, its quantity kinds (input and output text
  tokens, cached input tokens, input and output audio tokens, input and output audio milliseconds,
  input characters, requests, local compute milliseconds), the quantity source, and the
  `UsageRecorded` event; the duplex usage types map onto it.
- Emission in every adapter path: `inference-llm` from `llm`'s usage; the duplex bindings from
  their provider usage; STT and TTS from audio and text quantities; local adapters add measured
  compute time.
- Aggregation per run and per session in the facade, in its public event vocabulary.
- Pricing behind a feature, over `llm-cost`'s price documents; a cost record per usage record
  and a session total; currency and price version named in each.
- The replay recorder records usage records; replay does not compare them.

## Exit

- One test per adapter shows a usage record with the quantities its provider or backend reports,
  and `unknown` where it reports nothing.
- A voice-cascade run on the mocks yields one usage record per model call, a session total equal
  to their sum, and, with a price document, a cost total equal to the priced sum.
- A telemetry consumer that never reads loses no usage record.

## Out of scope

Provider price lookup, currency conversion, budgets and quota enforcement (P7).
