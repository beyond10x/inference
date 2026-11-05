---
format: aep.planning-md/3
id: story:deepgram-voice-agent-adapter
kind: story
status: proposed
title: Deepgram Voice Agent as a duplex binding
summary: A bundled hosted agent beside the native speech models in compare.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T13:07:48Z", actor: "human:timo", revision: 2}
---
## Context

A downstream adopter holds an account with Deepgram's Voice Agent API, a hosted STT, LLM and TTS
bundle behind one WebSocket session. Deepgram offers μ-law and A-law at 8 kHz end to end and an EU
endpoint. In the Voice Agent, non-English voices need a third-party TTS. On the public
live-conversation arena, the default Deepgram cascade ranks in the lower third.

inference binds three native speech models (OpenAI Realtime, GPT-Live, Nova Sonic) and no
bundled hosted agent.

## Planned scope

- A duplex binding of the `inference-duplex` contract for the Voice Agent session, in a new
  `adapters/inference-deepgram` crate:
  - its claims record, held equal to the code;
  - the protocol from Deepgram's public documentation, saved with its date;
  - `compare` gains it as a provider.

## Acceptance

- The duplex conformance procedure passes against a local fixture server that speaks the
  documented protocol, covering barge-in and a closed session.
- The live test is ignored and behind feature `live`. It needs a Deepgram Voice Agent key.
