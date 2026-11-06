---
format: aep.planning-md/3
id: story:deepgram-voice-agent-adapter
kind: story
status: implemented
title: Deepgram Voice Agent as a duplex binding
summary: A bundled hosted agent beside the native speech models in compare.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
- depends_on: story:deepgram-streaming-stt-adapter
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.lock
- confidence: cited
  path: adapters/inference-deepgram
- confidence: inferred
  path: adapters/inference-deepgram/Cargo.toml
- confidence: inferred
  path: adapters/inference-deepgram/records/claims-voice-agent.json
- confidence: inferred
  path: adapters/inference-deepgram/src/agent.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/config.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/credential.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/duplex.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/error.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/lib.rs
- confidence: inferred
  path: adapters/inference-deepgram/tests/claims_record.rs
- confidence: inferred
  path: adapters/inference-deepgram/tests/duplex.rs
- confidence: inferred
  path: adapters/inference-deepgram/tests/support/mod.rs
- confidence: inferred
  path: crates/inference-docs/src/adapters.rs
- confidence: inferred
  path: examples/realtime-voice/Cargo.toml
- confidence: cited
  path: examples/realtime-voice/examples/compare.rs
- confidence: inferred
  path: examples/realtime-voice/src/compare.rs
- confidence: inferred
  path: examples/realtime-voice/src/lib.rs
- confidence: inferred
  path: examples/realtime-voice/src/usage.rs
- confidence: inferred
  path: website/docs/guides/compare-voice-providers.md
- confidence: inferred
  path: website/docs/reference/adapters.md
- confidence: inferred
  path: website/docs/reference/crates.md
- confidence: inferred
  path: website/docs/reference/environment.md
revision: 31
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T13:07:48Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-05T16:11:01Z", actor: "human:operator", revision: 30}
- {from: "active", to: "implemented", at: "2026-11-06T10:49:22Z", actor: "human:operator", revision: 31, decided_on: {"recorded":{"test_result":2,"review_outcome":2}}}
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

## Scope

Derived 2026-11-05 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `adapters/inference-deepgram`, a new crate — cited (story, Planned scope)
- **Files:**
  - `examples/realtime-voice/examples/compare.rs` — cited (story: "`compare` gains it as a provider"): a `Provider` variant with `name()` and `topology()` arms (`Topology::Native`) and a match arm in `main`, as `Provider::Nova` has, once the 0.16.0 topology unit lands
  - `adapters/inference-deepgram/src/protocol.rs` — inferred: the protocol "saved with its date" in module docs, as `adapters/inference-elevenlabs/src/protocol.rs:5` and `adapters/inference-openai/src/live.rs:12` do
  - `adapters/inference-deepgram/src/duplex.rs` — inferred: the `NativeSpeechModel` binding, named after `inference-aws/src/duplex.rs`
  - `adapters/inference-deepgram/records/claims-voice-agent.json` — inferred name; the record is cited
  - `adapters/inference-deepgram/tests/claims_record.rs` — inferred
  - `adapters/inference-deepgram/tests/support/mod.rs` — inferred: the local fixture WebSocket server, modelled on `adapters/inference-elevenlabs/tests/support/mod.rs`
  - `adapters/inference-deepgram/tests/duplex.rs` — inferred: conformance against the fixture, barge-in and a closed session (acceptance cited)
- **Also likely:**
  - `adapters/inference-deepgram/Cargo.toml`, `src/lib.rs`, `src/config.rs` (the EU endpoint), `src/credential.rs`, `src/error.rs` — inferred, from the layout of `inference-elevenlabs`
  - `examples/realtime-voice/Cargo.toml`, `examples/realtime-voice/src/lib.rs` (a session config beside `nova_config`) — inferred
  - `examples/realtime-voice/src/compare.rs`, `src/usage.rs` — inferred: a billing value for a per-minute bundle
  - `Cargo.lock`, `crates/inference-docs/src/adapters.rs`, generated `website/docs/reference/adapters.md` and `crates.md` — inferred
- **Documents:** `website/docs/guides/compare-voice-providers.md`, `website/docs/reference/environment.md`, `CHANGELOG.md` — inferred. No `ess/` change: no domain names a provider — inferred
- **Confidence:** medium. The story names the crate, the contract and `compare` but no file
- **Would collide with:** story:deepgram-streaming-stt-adapter on the crate's `Cargo.toml`, `src/lib.rs`, `src/config.rs`, `src/credential.rs`, `src/error.rs`, `tests/claims_record.rs`, `tests/support/mod.rs` and possibly `src/protocol.rs`; the `compare` surface `examples/realtime-voice/{Cargo.toml,examples/compare.rs,src/compare.rs,src/lib.rs}` and `Cargo.lock` (cited from the 0.16.0 topology unit's diffstat); `crates/inference-docs/src/adapters.rs`; `website/docs/guides/compare-voice-providers.md`, `website/docs/reference/environment.md`, `CHANGELOG.md`
- **Safety fact:** the workspace takes members by glob (`Cargo.toml:3`), so the new crate needs no root manifest change; `native_duplex_evidence` (`crates/inference-duplex/src/conformance.rs:128`) is generic over `NativeSpeechModel`, so the contract crate stays unchanged
- **Not established:** how the Voice Agent bills and whether it reports usage; whether the two Deepgram bindings share one `protocol.rs`
