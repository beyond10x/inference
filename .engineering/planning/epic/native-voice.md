---
format: aep.planning-md/3
id: epic:native-voice
kind: epic
status: draft
title: Native and hybrid voice modalities (P5)
summary: A native speech model as one multiport node; hybrid audio-understanding composition; capabilities declared directly.
relations:
- derived_from: architecture-design:inference
- depends_on: epic:local-voice-cascade
- serves: vision:inference
revision: 2
---
## Context

Stage P5, "native and hybrid modalities" (`docs/design/inference.md:1163`): a multiport native
speech adapter and audio-aware model composition; exit "native voice graph passes declared
capability tests". A native audio-to-audio model is one multiport node, never three fake components
(§7.5, line 407; A-06, line 1142); topology D (§14.4, lines 790-801) and the hybrid C (§14.3,
lines 780-788). The candidate is Kyutai Moshi behind an adapter boundary (lines 926, 938), with
code and weight licences as separate manifest entries (line 929).

What evidence lets an adapter claim `native_duplex` is undecided:
`decision-blocker:native-duplex-evidence` (Q15, line 1240).

## Outcome

One native speech adapter is a single multiport node with synchronized audio and optional text
ports, its capabilities declared directly and passed by the conformance tests, composed in a graph
with optional transcript and tool sidecars that are real nodes with their own cost.

## Exit

- A-06 (line 1142).
- The adapter's record claims `native_duplex` only with the evidence the blocker names.
- `inference-duplex` split out (§5.3) with the §7.4 concepts (lines 392-403) declared in the
  specification first.

## Not decomposed

Stories wait for the cascade to land and for the blocker to clear.
