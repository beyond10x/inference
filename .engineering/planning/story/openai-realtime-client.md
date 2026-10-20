---
format: aep.planning-md/3
id: story:openai-realtime-client
kind: story
status: implemented
title: OpenAI Realtime client with offline and live tests
summary: adapters/inference-openai speaks the Realtime WebSocket API; offline replay tests and an ignored live test.
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: adapters/inference-openai
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T20:56:23Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T20:56:23Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-20T01:33:46Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

The operator asked on 2026-10-19 for a test against the OpenAI Realtime speech-to-speech model,
urgently, in this repository, with each provider in its own crate. The design puts hosted
integrations under `adapters/` (`docs/design/inference.md:203-213`, `inference-openai`) and requires
them to be enabled explicitly (lines 903-904). This story is the provider client and its tests; the
binding to the `inference-duplex` contract is `story:openai-realtime-adapter`, after
`story:native-speech-contract`.

Secrets: the client never reads a key itself. The caller injects a credential resolver; the crate
takes `b10x-llm-credentials`' `SecretResolver` (beyond10x `llm`, by Git tag), whose environment
resolver serves the tests and whose Secrets adapter serves a deployment. No key, account name or
host path is committed.

## Planned scope

- `adapters/inference-openai` (crate `inference-openai`), workspace member through
  `members = ["crates/*", "examples/*", "adapters/*"]`.
- A Realtime client over WebSocket: open a session with the model id and session configuration
  (voice, instructions, audio formats, server turn detection on or off, tools); send input audio
  (PCM16 at the rate the API takes, resampled from `inference-audio` frames where needed, or
  G.711 mu-law at 8 kHz passed through), commit, create response (with automatic responses off),
  cancel, truncate the output item at a played position, text items and tool results; server or
  semantic turn detection, and auto-interrupt off; a read timeout that yields a typed stall error; receive server events as typed values (audio delta,
  transcript deltas, input transcript, speech started and stopped, response done with usage, function
  call, error). Unknown server events are kept as unknown, never dropped silently.
- Offline tests against a local WebSocket server in the test, replaying event shapes written from
  the published API reference: session setup, a text-in audio-out response, barge-in cancel, a
  function call answered, an error event. No network.
- A live test, `#[ignore]` and behind a `live` feature, that reads the key through the environment
  resolver (`OPENAI_API_KEY`) and the model id from `INFERENCE_OPENAI_REALTIME_MODEL` (default the
  current GA realtime model): it sends one text item and asserts audio deltas, an output transcript
  and `response.done`, then sends a short committed speech clip and asserts an input transcript.
  The speech clip is generated, not recorded from a person, and its generator is named.

## Acceptance

`cargo test -p inference-openai --all-features` passes offline with no key present. With a key,
`cargo test -p inference-openai --features live -- --ignored` completes one text-in audio-out
response and one audio-in response, printing the usage the server reported.

## Out of scope

The `inference-duplex` contract and graph nodes; WebRTC; SIP.
