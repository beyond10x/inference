---
format: aep.planning-md/3
id: story:cartesia-streaming-tts-adapter
kind: story
status: active
title: Cartesia Sonic streaming TTS as a hosted TTS adapter
summary: The lowest-latency hosted voice for a cascade, with phone-rate output.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T17:26:37Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-06T17:26:37Z", actor: "human:operator", revision: 3}
---
## Context

A downstream adopter's November 2026 survey names Cartesia Sonic 3.6 as the lowest-latency TTS for
real-time agents (under 90 ms, vendor figure), fourth on the blind TTS arena, with 44 languages and
μ-law and A-law at 8 kHz on the Sonic 3 API (not confirmed for 3.6). On the live speech-agent arena
a cascade built on it ranks first. The survey's suggested cascade baseline names ElevenLabs v4 Turbo
or Cartesia Sonic 3.6 for the voice.

inference has one hosted TTS adapter, ElevenLabs. Cut from story:more-hosted-voice-adapters.

## Planned scope

- A streaming TTS binding of the `inference-tts` contract over Cartesia's documented WebSocket API,
  in a new `adapters/inference-cartesia` crate:
  - its claims record, held equal to the code;
  - the protocol from Cartesia's public documentation, saved with its date;
  - the cascade in `compare` can name it as its voice.

## Acceptance

- The backend-neutral TTS checks pass against a local fixture server that speaks the documented
  protocol, including a cancel mid-utterance.
- A fixture run asks for μ-law 8 kHz output and receives it.
- The live test is ignored and behind feature `live`. It needs a Cartesia API key.
