---
format: aep.planning-md/3
id: story:realtime-usage-cached-audio-input
kind: story
status: active
title: Usage reports cached audio input tokens when the provider sends them
summary: A cached_input_audio_tokens quantity kind, read from response.done, unknown when not sent.
tags:
- downstream-adopter
- openai-realtime
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-openai/src/duplex.rs
- confidence: cited
  path: adapters/inference-openai/src/event.rs
- confidence: cited
  path: crates/inference-core/src/usage.rs
- confidence: inferred
  path: crates/inference-facade/src/cost.rs
- confidence: cited
  path: ess/domains/usage.yaml
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-11-03T15:55:03Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-03T15:55:03Z", actor: "human:timo", revision: 3}
---
## Context

A downstream adopter asked that usage report cached audio input tokens when the provider sends
them. The Realtime `response.done` usage carries, per the published server events reference,
`usage.input_token_details.cached_tokens_details` with `text_tokens` and `audio_tokens` (to be
confirmed against the reference page by the implementor; the offline tests copy the event from
it). On 0.10.0 the client reads only `input_token_details.cached_tokens`
(`adapters/inference-openai/src/event.rs:402`, verified), so the audio share of the cached input
is lost, and with it the price difference between cached audio and cached text input. Neither
`inference.usage.QuantityKind` (`ess/domains/usage.yaml:53-65`) nor
`inference_core::QuantityKind` (`crates/inference-core/src/usage.rs:55-78`) has a kind for it.

## Planned scope

- `ess/domains/usage.yaml`: a `cached_input_audio_tokens` variant of
  `inference.usage.QuantityKind` (audio input tokens served from the provider's cache, counted
  inside both `cached_input_tokens` and `input_audio_tokens`, never added to either), with a
  `read from:` line; `ess specify validate --path ess` passes.
- `crates/inference-core/src/usage.rs`: the variant, `QuantityKind::ALL` in the spec's order, and
  every exhaustive match on the kind in the workspace.
- `adapters/inference-openai/src/event.rs`: `Usage::cached_input_audio_tokens: Option<u64>`, read
  from `input_token_details.cached_tokens_details.audio_tokens`, `None` when the server leaves
  it out or sends it in another shape; `Display` shows it.
- `adapters/inference-openai/src/duplex.rs`: the binding's response quantities carry the new kind,
  provider-reported when sent and unknown when not.
- `crates/inference-facade/src/cost.rs`: a price binding that charges the new kind gets the same
  treatment as any kind `llm-cost` has no unit for (a price-unknown line), never a silent zero.
- Out of scope: GPT-Live (billed per voice second; its backend usage is Responses text usage).

## Acceptance

Named scenarios, offline:

- `response_done_reads_cached_audio_input_tokens` (`adapters/inference-openai`): a `response.done`
  whose usage carries `input_token_details.cached_tokens_details.audio_tokens` gives `Usage`
  with that count in `cached_input_audio_tokens`.
- `response_done_without_cached_details_keeps_cached_audio_unknown`: a `response.done` without
  `cached_tokens_details`, or without its `audio_tokens`, or with it in another shape, gives
  `cached_input_audio_tokens: None`, never `Some(0)`.
- `realtime_duplex_records_cached_audio_input_tokens` (feature `duplex`): the usage the binding
  records for a response carries a `cached_input_audio_tokens` quantity, provider-reported with
  the value when the server sent it and unknown with no value when it did not.
- `quantity_kind_all_matches_the_specification` (`crates/inference-core`, existing or new): the
  kinds of `QuantityKind::ALL` serialize to the variants of `inference.usage.QuantityKind`, in
  order, including `cached_input_audio_tokens`.
- `ess specify validate --path ess` passes.
