---
format: aep.planning-md/3
id: story:contract-usage-and-naming-drift
kind: story
status: active
title: Contract crates agree on usage counting, event names and the design's model descriptors
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
- serves: vision:inference
scope:
- confidence: inferred
  path: crates/inference-core/src/model.rs
- confidence: inferred
  path: crates/inference-core/src/usage.rs
- confidence: inferred
  path: crates/inference-duplex/src/ledger.rs
- confidence: inferred
  path: crates/inference-stt/src/lib.rs
- confidence: inferred
  path: crates/inference-tts/src/lib.rs
- confidence: inferred
  path: ess/domains/audio.yaml
- confidence: inferred
  path: ess/domains/model.yaml
- confidence: inferred
  path: ess/domains/usage.yaml
revision: 11
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T00:41:04Z", actor: "human:timo", revision: 10}
- {from: "proposed", to: "active", at: "2026-11-06T00:41:05Z", actor: "human:timo", revision: 11}
---
## Context

From review-result:full-codebase-2026-11-06 (findings N6, N12, N13, N14, N15), at release 0.16.0:

- N6 `crates/inference-duplex/src/ledger.rs:1318-1330` vs `:1353-1369`: `response_usage` still records `ResponseUsageRecorded` after the response's usage record, where `response_usage_reported` refuses it.
- N12 `crates/inference-core/src/usage.rs:548-565`, `:600-602`: `UsageTotal::add(&[], false)` counts a partial record but no kind, so `complete()` stays true, against the module doc.
- N13 `crates/inference-stt/src/lib.rs:784` vs `crates/inference-tts/src/lib.rs:783-787`: STT counts audio offered, TTS counts characters taken.
- N14 `crates/inference-tts/src/lib.rs:313-319`: `SynthesisEvent::name()` returns a type name (`inference.audio.AudioFrame`) where `audio.yaml` declares no event.
- N15 `crates/inference-core/src/model.rs:503-518`, `:326-339`: descriptors cannot express design §8.2 and §7.3 properties (voices, codecs, context size, duration limits, state scope, rate limits); `ess/domains/model.yaml` declares none of them.

## Planned scope

- ESS first: `ess/domains/{duplex,usage,stt,audio,model}.yaml` state each rule below, validated with `ess specify validate --path ess`.
- `response_usage` refuses after the usage record, as `response_usage_reported` does.
- A partial record with no quantities makes `complete()` false.
- STT counts audio taken by the backend, as TTS counts characters taken.
- `audio.yaml` declares the frame event the TTS handle names, or the name changes to a declared event.
- Model descriptors gain the design's properties as optional, additive fields; existing adapters compile unchanged.

## Acceptance

- `response_usage` after the usage record is refused and recorded as a drop.
- `UsageTotal::add(&[], false)` leaves `complete()` false.
- A recogniser failed before taking its tail reports only the audio it took.
- `SynthesisEvent::name()` names an event `ess/` declares.
- A descriptor carrying voices, context size and a rate limit round-trips through its wire form; `ess specify validate` passes.
