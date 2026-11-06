---
format: aep.planning-md/3
id: story:deepslate-realtime-adapter
kind: story
status: implemented
title: Deepslate Opal as a duplex binding
summary: An EU-hosted speech-to-speech provider in compare.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T17:26:36Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-06T17:26:37Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-06T18:22:07Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Context

Deepslate offers Opal, a speech-to-speech model hosted in the EU, with a public WebSocket API. A
downstream adopter's first impression was that its barge-in "seems pretty good". On public
benchmarks Opal sits mid-table:

- conversational dynamics 85.7%;
- one real-call reliability test 57.3%.

Its distinct case is EU-only processing.

## Planned scope

- A duplex binding over Opal's documented WebSocket protocol, in its own adapter crate, with its
  claims record and `compare` provider.

## Acceptance

- The duplex conformance procedure passes against a local fixture server.
- The live test is ignored and behind feature `live`. It needs a Deepslate API key.
