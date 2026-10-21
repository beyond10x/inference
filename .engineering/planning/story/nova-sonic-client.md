---
format: aep.planning-md/3
id: story:nova-sonic-client
kind: story
status: implemented
title: Amazon Nova Sonic client with offline and live tests
summary: adapters/inference-aws speaks Bedrock's bidirectional stream for Nova Sonic; offline tests and an ignored live test.
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: adapters/inference-aws
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T20:56:23Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T20:56:23Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-21T09:07:02Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1,"review_outcome":6}}}
---
## Context

The operator asked on 2026-10-19 for a test against Amazon Nova Sonic, the speech-to-speech model on
AWS Bedrock, urgently, in this repository, with each provider in its own crate. The design puts
hosted integrations under `adapters/` (`docs/design/inference.md:203-213`) and requires them to be
enabled explicitly (lines 903-904). This story is the provider client and its tests; the binding to
the `inference-duplex` contract is `story:nova-sonic-adapter`, after `story:native-speech-contract`.

Secrets: AWS credentials come from the AWS SDK's default provider chain (environment, profile, SSO),
which the caller can replace with its own provider; the crate never reads a key itself. No account,
profile name, key or host path is committed.

## Planned scope

- `adapters/inference-aws` (crate `inference-aws`), workspace member through
  `members = ["crates/*", "examples/*", "adapters/*"]`.
- A Nova Sonic client over Bedrock's bidirectional stream (`InvokeModelWithBidirectionalStream`,
  through `aws-sdk-bedrockruntime`): session start with inference configuration, prompt start with
  voice, audio formats and tools, system prompt, audio input content (PCM16 at the rate the model
  takes; say whether 8 kHz telephony audio is accepted or must be resampled, from the reference),
  tool results, and the end events; a read timeout that yields a typed stall error; output events as typed values (text with role and
  stage, audio output, tool use, content start and end, completion, usage, error). Unknown events
  are kept as unknown.
- Offline tests of the event encoding and decoding and of the session sequence, against event shapes
  written from the published Nova Sonic reference. No network.
- A live test, `#[ignore]` and behind a `live` feature, using the default credential chain, region
  from `AWS_REGION` and model id from `INFERENCE_NOVA_SONIC_MODEL` (default the current Nova Sonic
  id): it streams a short generated speech clip and asserts the user transcript, an assistant text
  and audio output, and the completion. The clip's generator is named.

## Acceptance

`cargo test -p inference-aws --all-features` passes offline with no credential present. With
credentials and model access in the region, `cargo test -p inference-aws --features live --
--ignored` completes one audio-in audio-out exchange, printing the usage the service reported.

## Out of scope

The `inference-duplex` contract and graph nodes; any AWS service other than Bedrock runtime.
