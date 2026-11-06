---
format: aep.planning-md/3
id: story:voice-topology-latency-per-step
kind: story
status: implemented
title: Cascade and native speech models in one latency report, per step
summary: Time to first audio and turn latency for both topologies, with per-step latency for the cascade.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T13:07:48Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-05T13:07:48Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-06T17:12:25Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":2,"review_outcome":3}}}
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

## Attempt in 0.16.0

The work of this attempt is on branch `impl/voice-topology-latency-per-step` at `2ff98aa`. It is
not merged and not released.

## What the attempt built

`compare` reports a native arm and a cascade arm in one table, with the measures they share named
the same: first audio, response end (the turn latency), and barge-in to stop. The cascade arm adds
its steps: final transcript, first token and TTS first audio.

The fixture run passes:

- each step is within one 20 ms input frame of its scripted delay;
- a mutant that adds one step's delay to the next fails;
- a mutant that gives every step the same instant fails;
- with no hosted TTS configured, the cascade is skipped and the native arm still reports.

## Why it left the 0.16.0 wave

The adversary pass found one blocker, and the unit's one correction could not clear it.

While a binding's `next()` blocks, the cascade arm sends no input. The LLM's generation handle
blocks while it works, and ElevenLabs blocks while a segment is spoken. The input frames that fall
due meanwhile then go out in one burst, so the barge-in is stamped late and barge-in to stop reads
10 ms where 30 ms is right. The local cascade's pipeline keeps taking input during such a block.

The correction fixed the other findings:

- the topology is now given explicitly, so a cascade with no run is no longer labelled native;
- the cascade uses the native client playout model;
- the "playout start" column is dropped, because without a device it always equals TTS first
  audio;
- the environment table now names `compare`.

It did not take the blocker's fix. The adversary's case advances the shared fake clock 200 ms from
inside the LLM, so any loop that reads the clock next sees every frame of those 200 ms as late. With
worker threads, the result would race the worker's jump.

## The design for the next attempt

1. In the adversary case's slow LLM, replace the clock advance with a future that stays pending
   until the caller's clock reaches the delay. This is a test-side change made by the side whose
   code the case checks, so before it lands, a throwaway mutant must show that the changed case
   still goes red on the burst.
2. Run the recogniser, the LLM and the synthesiser on scoped worker threads that report to the
   arm's loop over channels. The loop paces input and stamps each event as it arrives.
3. Two pacing modes:
   - live waits on the channel until the next frame is due;
   - simulated advances the clock only when every worker is idle or parked on a pending future.
