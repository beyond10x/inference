---
format: aep.planning-md/3
id: decision-blocker:native-duplex-evidence
kind: decision-blocker
status: open
title: Nobody has decided what evidence lets an adapter claim native_duplex (Q15)
relations:
- blocks: epic:native-voice
- blocks: epic:duplex-research
revision: 1
---
## Question

Open question 15 of the design: "Which conformance evidence is required before claiming
`native_duplex`?" (`docs/design/inference.md:1240`). `native_duplex` means the model and serving
stack compute on simultaneous inbound and outbound streams (line 422); a coordinator that restarts
ordinary LLM calls gives an orchestrated duplex experience and must not claim it (lines 52-54);
A-07 requires the two to be reported as different effective capabilities (line 1143).

## What it stops

native-voice (an adapter's record may not claim `native_duplex` without the evidence) and
duplex-research (A-07 cannot be demonstrated without a rule for what counts).

## Default on silence

One conformance scenario: with one model instance and one open generation, an input event is
accepted after output has begun, and an output event that depends on that input is emitted before
the generation completes, with no restart of the generation and no second generation id. An adapter
passing it may claim `native_duplex`; one that passes only by restarting claims
`concurrent_input_output` at most. The scenario is declared in `ess/domains/model.yaml` or a
duplex domain before the first adapter claims it.

## Clears when

The owner states the required evidence, or accepts the default, and it is written into this body
and into the specification.
