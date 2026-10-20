---
format: aep.planning-md/3
id: story:openai-realtime-adapter
kind: story
status: implemented
title: OpenAI Realtime as an inference-duplex native speech model
summary: The Realtime client implements the duplex contract and passes its conformance cases offline.
relations:
- decomposes: epic:native-voice
- serves: vision:inference
- depends_on: story:native-speech-contract
- depends_on: story:openai-realtime-client
scope:
- confidence: inferred
  path: adapters/inference-openai
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:12:12Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T22:12:12Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-20T01:33:47Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

Binds a hosted realtime provider client to the `inference-duplex` contract of
`story:native-speech-contract`, so the provider is a native speech model the graph can use like any
other, and the provider's live test runs through the contract instead of the raw client. Planned
after the contract and the provider client land.

## Acceptance

The provider implements the `inference-duplex` native speech model; the duplex conformance cases
that run against the testkit mock run against an offline replay of the provider; the live test
(`#[ignore]`, `live` feature) runs one audio-in audio-out session through the contract.
