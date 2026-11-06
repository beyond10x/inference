---
format: aep.planning-md/3
id: story:adapter-descriptor-carrier
kind: story
status: active
title: The adapter carries its AdapterDescriptor
summary: DeclareAdapter and Adapter carry the AdapterDescriptor added in 0.19.0 (N15); every DeclareAdapter literal is updated
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T19:11:13Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-11-06T19:11:13Z", actor: "human:timo", revision: 5}
---
# Story: the adapter carries its AdapterDescriptor

## Outcome

An application reading a declared `inference.model.Adapter` sees the adapter's voices, codecs,
context size, duration limits, token budget, state scope and rate limits on the adapter itself.

## Context

0.19.0 (finding N15 of review-result:full-codebase-2026-11-06) added `AdapterDescriptor` and
`RateLimit` to `inference-core` (`crates/inference-core/src/model.rs:467-507`) and
`ess/domains/model.yaml:127-150`, marked UNMAPPED for its carrier: `Adapter` or `CapabilityRecord`.

Decision, taken by the session that owns this repository (2026-11-06): **`Adapter` carries it.**
The design lists these descriptors beside the adapter's capabilities, and capabilities are the
adapter's (`Adapter.capabilities`); a capability record is per hardware target, and a voice list or
a rate limit does not change with the hardware. The descriptor was held back only because adding a
field breaks every `DeclareAdapter` literal; backwards compatibility is no longer kept (AGENTS.md,
2026-11-06).

## Acceptance

- `ess/domains/model.yaml`: `DeclareAdapter`'s input and the entity `Adapter` carry
  `descriptor: inference.model.AdapterDescriptor`; the UNMAPPED marker on the carrier is removed;
  `ess specify validate --path ess` passes and the synthesized suite covers the field.
- `inference-core`: `DeclareAdapter` has a `descriptor: AdapterDescriptor` field and `Adapter` a
  `descriptor()` accessor; a test declares an adapter with a non-empty descriptor and reads it back,
  and the `AdapterDeclared` event or the view the specification names carries it.
- Every `DeclareAdapter` literal in the repository is updated (107 literals in 97 files on 0.22.0);
  the hosted adapters whose claims records name voices or codecs fill them in where the code
  already knows them, others use `AdapterDescriptor::default()`.
- `CHANGELOG.md` `## [Unreleased]` has a `Breaking` line.

## Out of Scope

- A vocabulary for `RateLimit.unit` or `state_scope`.
