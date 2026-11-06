---
format: aep.planning-md/3
id: story:phonic-realtime-adapter
kind: story
status: active
title: Phonic 1 as a duplex binding
summary: The top-ranked speech-to-speech model on two independent boards, beside the native speech models in compare.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T17:26:37Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-06T17:26:37Z", actor: "human:timo", revision: 3}
---
## Context

A downstream adopter's November 2026 survey ranks Phonic 1 first on two independent boards: the
Artificial Analysis speech-to-speech index (87.1) and one real-call reliability test (90.2%, three
runs of 82 scenarios over live calls). It lists 51 languages and SIP trunking. The same survey notes
that Phonic 1 has no conversational-dynamics score, so its turn-taking is unmeasured there, and that
its EU data residency is unverified.

inference binds three native speech models (OpenAI Realtime, GPT-Live, Nova Sonic). Cut from
story:more-hosted-voice-adapters.

## Planned scope

- A duplex binding of the `inference-duplex` contract over Phonic's documented realtime WebSocket
  API, in a new `adapters/inference-phonic` crate:
  - its claims record, held equal to the code;
  - the protocol from Phonic's public documentation, saved with its date;
  - `compare` gains it as a provider.

## Acceptance

- The duplex conformance procedure passes against a local fixture server that speaks the
  documented protocol, covering barge-in and a closed session.
- The live test is ignored and behind feature `live`. It needs a Phonic API key.
