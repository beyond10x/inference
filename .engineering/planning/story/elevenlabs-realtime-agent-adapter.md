---
format: aep.planning-md/3
id: story:elevenlabs-realtime-agent-adapter
kind: story
status: draft
title: ElevenLabs realtime conversation as a duplex binding
summary: Eleven Realtime as a native speech provider in compare.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 1
---
## Context

A downstream adopter names ElevenLabs as the voice "most people mention". ElevenLabs sells a
speech-to-speech model, Eleven Realtime v1 Max, through its conversational agents. On public
indexes it reports 0.57 s to first audio and ranks mid-table on conversational dynamics.

## Planned scope

- A duplex binding for ElevenLabs' realtime conversational session in `adapters/inference-elevenlabs`
  (story:elevenlabs-streaming-tts-adapter creates the crate):
  - its claims record;
  - the protocol from ElevenLabs' public documentation, saved with its date;
  - `compare` gains it as a provider.

## Acceptance

- The duplex conformance procedure passes against a local fixture server, covering barge-in and a
  closed session.
- The live test is ignored and behind feature `live`. It needs an ElevenLabs API key with agent
  access.
