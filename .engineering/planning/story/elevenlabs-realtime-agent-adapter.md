---
format: aep.planning-md/3
id: story:elevenlabs-realtime-agent-adapter
kind: story
status: active
title: ElevenLabs realtime conversation as a duplex binding
summary: Eleven Realtime as a native speech provider in compare.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.lock
- confidence: cited
  path: adapters/inference-elevenlabs
- confidence: inferred
  path: adapters/inference-elevenlabs/Cargo.toml
- confidence: inferred
  path: adapters/inference-elevenlabs/records/claims-agent.json
- confidence: inferred
  path: adapters/inference-elevenlabs/src/agent.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/src/agent_duplex.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/src/lib.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/tests/agent_duplex.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/tests/agent_duplex_live.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/tests/claims_record.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/tests/support/mod.rs
- confidence: inferred
  path: crates/inference-docs/src/adapters.rs
- confidence: inferred
  path: crates/inference-docs/src/status.rs
- confidence: cited
  path: examples/realtime-voice/Cargo.toml
- confidence: cited
  path: examples/realtime-voice/examples/compare.rs
- confidence: inferred
  path: examples/realtime-voice/src/lib.rs
- confidence: inferred
  path: website/data/status.json
- confidence: inferred
  path: website/docs/guides/compare-voice-providers.md
- confidence: inferred
  path: website/docs/reference/adapters.md
- confidence: inferred
  path: website/docs/reference/crates.md
- confidence: inferred
  path: website/docs/reference/environment.md
- confidence: inferred
  path: website/docs/status.mdx
revision: 27
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T13:07:49Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-05T16:11:00Z", actor: "human:timo", revision: 27}
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

## Scope

Derived 2026-11-05 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `adapters/inference-elevenlabs`. A new duplex binding beside the TTS
  synthesiser, behind a new feature (`duplex`, on `inference-duplex` as `inference-aws` does) — cited for the crate (story "Planned scope"), inferred for the feature
- **Files:** `adapters/inference-elevenlabs/src/lib.rs` (module list, re-exports, `claims`) — inferred
- **Files:** `adapters/inference-elevenlabs/Cargo.toml` (feature `duplex`, `inference-duplex` dependency, `inference-testkit` feature `duplex`) — inferred, modelled on `adapters/inference-aws/Cargo.toml`
- **Files:** a new agent-session module pair, such as `adapters/inference-elevenlabs/src/agent.rs` (wire protocol, the public documentation saved with its date, as `src/protocol.rs` does for TTS) and `adapters/inference-elevenlabs/src/agent_duplex.rs` (the `NativeSpeechModel`) — inferred names; the dated protocol is cited
- **Files:** `adapters/inference-elevenlabs/records/claims-agent.json` and `adapters/inference-elevenlabs/tests/claims_record.rs` — cited for "its claims record", inferred for the file name
- **Files:** `adapters/inference-elevenlabs/tests/agent_duplex.rs` (fixture-server conformance through `inference_duplex::conformance::native_duplex_evidence`, barge-in and closed session) and `adapters/inference-elevenlabs/tests/agent_duplex_live.rs` (ignored, feature `live`) — cited for the acceptance, inferred for the names
- **Files:** `adapters/inference-elevenlabs/tests/support/mod.rs` (the WebSocket fixture server) — inferred: extended, or a second fixture module added
- **Files:** `examples/realtime-voice/examples/compare.rs` (`enum Provider`, `name`, `topology`, the provider construction) — cited ("`compare` gains it as a provider"), read from the 0.16.0 topology unit's branch
- **Files:** `examples/realtime-voice/src/lib.rs` (a session config beside `nova_config` and `openai_live_config`) — inferred
- **Files:** `examples/realtime-voice/Cargo.toml` (the `inference-elevenlabs` dependency gains feature `duplex`) — cited from the topology unit's branch
- **Files:** `Cargo.lock` — inferred
- **Symbols:** `inference_duplex::NativeSpeechModel`, `inference_duplex::conformance::native_duplex_evidence` (`crates/inference-duplex/src/conformance.rs:128`), `realtime_voice::compare::run` (generic over `NativeSpeechModel`) — cited
- **Also likely:** `crates/inference-docs/src/adapters.rs`, `crates/inference-docs/src/status.rs` and the regenerated `website/docs/reference/adapters.md`, `website/docs/reference/crates.md`, `website/docs/status.mdx`, `website/data/status.json` — inferred: `task docs-check` fails on a stale generated file
- **Documents:** `website/docs/guides/compare-voice-providers.md`, `website/docs/reference/environment.md`, `CHANGELOG.md` under Unreleased — inferred
- **Documents:** `ess/` unchanged — inferred: an adapter of the existing duplex domain, no new noun
- **Confidence:** medium. The story names the crate, the claims record, the dated protocol and `compare`; file names inside the crate are read from the sibling adapters
- **Would collide with:** any unit in `adapters/inference-elevenlabs` (story:elevenlabs-multi-context-tts-connection shares `src/lib.rs`, `Cargo.toml`, `tests/support/mod.rs`, `records/`, `tests/claims_record.rs`); `examples/realtime-voice`'s `compare`; `crates/inference-docs/src/{adapters,status}.rs` and their generated pages; `Cargo.lock`. Sequence after the 0.16.0 topology unit merges
- **Not established:** ElevenLabs' agent WebSocket protocol (signed URL or agent id, reuse of `src/connection.rs`); whether the binding claims `native_duplex`; whether `live` implies `duplex`; whether the agent accepts `compare`'s 24 kHz fixture or needs resampling
