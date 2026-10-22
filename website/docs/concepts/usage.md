---
title: Usage and cost
sidebar_label: Usage
sidebar_position: 8
description: One usage record per model call on the run's reliable path, quantities that stay unknown rather than zero, totals per run and session, and cost from a price document you supply.
lede: Every STT, LLM, TTS and native speech call reports what it consumed, and a count nobody reported stays unknown.
source: crates/inference-core (usage.rs), crates/inference-stt, crates/inference-tts, crates/inference-llm, crates/inference-duplex, crates/inference-facade (usage.rs, cost.rs), ess/domains/usage.yaml, CHANGELOG.md 0.6.0 and 0.7.0
---

# Usage and cost

Every model call in a run, STT, LLM, TTS or native speech, hosted or local, emits one usage record
on the run's egresses, the reliable path of committed facts. Usage never depends on telemetry,
which drops what its consumer does not take.

## The usage record

A `UsageRecord` (`inference.usage`, in `inference-core`) names who made the call (adapter, model,
run, node, session, speech session, response where there is one), when (start and end on the wall
clock), whether the provider marked its counts final, and a list of quantities. Each quantity has
a kind and a source:

| Kinds | Sources |
|---|---|
| input and output text tokens, cached input tokens, cache-creation input tokens, input and output audio tokens, input and output audio milliseconds, input characters, requests, local compute milliseconds | `provider_reported`, `measured`, `unknown` |

**Unknown is never zero.** A quantity's value is absent exactly when its source is `unknown`; a
record whose value and source disagree is refused (`QuantityMismatch`). Cached and cache-creation
input tokens are counted inside the input tokens, never added to them.

## Who records it

| Call | What it records | Since |
|---|---|---|
| LLM (`inference-llm`) | one record per `llm` turn, from llm's usage, on a pipeline's `usage` egress | 0.6.0 |
| native speech (`inference-duplex`) | one per response once its usage is settled, one per reported transcription, and usage no response owns as a session record before the session closes or fails; OpenAI Realtime, GPT-Live (billed session seconds) and Nova Sonic report through it | 0.7.0 |
| STT (`inference-stt`) | a recognition ends with `TranscriptEvent::Usage`: input audio counted from the frames offered, one request, and local compute where the adapter measures it (sherpa-onnx and whisper.cpp do) | 0.7.0 |
| TTS (`inference-tts`) | a synthesis ends with `SynthesisEvent::Usage`: characters taken, output audio, one request and local compute where measured | 0.7.0 |
| LLM, cut short | a cancelled or superseded generation keeps the usage llm reported for it | On main, in the next release |

A failed call is recorded with `final: false`, and its counts are a lower bound.

## Totals

On main, in the next release. A `UsageTotal` adds, per quantity kind, the known values and counts
the unknown ones apart, and is complete for a kind only when no quantity of that kind was unknown.
The [session facade](session.md) keeps one per run and one over the session. A run's total
includes the usage of its subgraphs whose `usage` port has an edge; the record still names the
child run it was made in, and is counted once.

## Cost

On main, in the next release, behind the facade's `cost` feature. A cost record prices one usage
record against one price document the caller supplies: its currency, revision and digest, one line
per charged quantity kind with an exact decimal amount or the reason it is unknown, and totals that
keep the unknown apart from the known. The prices are [llm](https://beyond10x.github.io/llm/)'s
`llm-cost` price book. No provider price is looked up and no currency converted. A usage record of
an adapter the pricing does not bind is priced as unknown, never skipped and never zero.

Released examples print usage and cost today without the facade: see
[Estimate cost from a price table](../guides/estimate-cost-from-a-price-table.md).
