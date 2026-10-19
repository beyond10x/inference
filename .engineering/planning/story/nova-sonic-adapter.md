---
format: aep.planning-md/3
id: story:nova-sonic-adapter
kind: story
status: draft
title: Amazon Nova Sonic as an inference-duplex native speech model
summary: The Nova Sonic client implements the duplex contract and passes its conformance cases offline.
relations:
- decomposes: epic:native-voice
- serves: vision:inference
- depends_on: story:native-speech-contract
- depends_on: story:nova-sonic-client
scope:
- confidence: inferred
  path: adapters/inference-aws
revision: 2
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
