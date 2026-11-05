---
format: aep.planning-md/3
id: story:voice-topology-latency-per-step
kind: story
status: active
title: Cascade and native speech models in one latency report, per step
summary: Time to first audio and turn latency for both topologies, with per-step latency for the cascade.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T13:07:48Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-05T13:07:48Z", actor: "human:operator", revision: 3}
---
## Context

A downstream adopter wants to compare voice-agent topologies, a cascade (STT, LLM, TTS) against
native speech-to-speech models, on time to first audio and turn latency per step. A public
real-call benchmark finds a classic cascade close to the best speech-to-speech models on
reliability, so the comparison is open.

`realtime-voice`'s `compare` example measures native models only:

- end of speech to first audio, first transcript and response end;
- barge-in to output stop.

It lists the cascade as skipped. Nothing reports the cascade's own steps: end of speech to final
transcript, to the LLM's first token, to the TTS's first audio and to playout.

## Planned scope

- One latency report for both topologies, with the same names for the shared measures (end of
  speech to first audio, turn latency) and per-step measures where a topology has steps.
- `compare` gains a cascade arm built from configured STT, LLM and TTS bindings.
- If the report is a new noun, its ESS domain comes first.

## Acceptance

- A fixture run with scripted bindings and known delays yields each step's latency within a
  stated tolerance, for a cascade and for a native model, in one report.
- With no hosted TTS configured, the cascade arm says so and is skipped, as a provider is today.
