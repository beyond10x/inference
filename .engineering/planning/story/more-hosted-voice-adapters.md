---
format: aep.planning-md/3
id: story:more-hosted-voice-adapters
kind: story
status: draft
title: Further hosted voice adapters from the November 2026 survey
summary: The candidates a shortlist may pick, each cut into its own story when picked.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 1
---
## Context

A downstream adopter's survey of voice models (November 2026) names further hosted candidates
that inference does not bind:

- speech-to-speech models:
  - Phonic 1, first on the public speech-to-speech index and on one real-call reliability test;
  - Grok Voice 3.0;
  - Gemini 3.8 Live, with the most languages and no native SIP;
  - Qwen Audio Realtime, second on conversational dynamics, with no EU region;
  - StepAudio 3 Realtime, first on the reasoning and conversational-dynamics boards;
- TTS:
  - Cartesia Sonic 3.6, the lowest latency for real-time agents;
  - Gemini 3.8 Flash TTS;
  - Azure Speech neural voices, the broadest catalogue;
- Azure's hosting of OpenAI's realtime models.

## Planned scope

One story per binding, cut from this one once the adopter's shortlist is known. Each is a hosted
adapter with its claims record, fixture tests against the documented protocol, and an ignored
live test behind feature `live`.

## Acceptance

Each binding cut from here passes the backend-neutral checks of its contract against a local
fixture server.
