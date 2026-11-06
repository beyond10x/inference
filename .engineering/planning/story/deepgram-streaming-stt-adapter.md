---
format: aep.planning-md/3
id: story:deepgram-streaming-stt-adapter
kind: story
status: implemented
title: Deepgram streaming STT as a hosted STT adapter
summary: A hosted STT layer for a phone-audio cascade.
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
  path: adapters/inference-deepgram
- confidence: cited
  path: adapters/inference-deepgram/Cargo.toml
- confidence: cited
  path: adapters/inference-deepgram/examples/live.rs
- confidence: cited
  path: adapters/inference-deepgram/records/claims-stt.json
- confidence: inferred
  path: adapters/inference-deepgram/src/config.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/credential.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/error.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/lib.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/listen.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/recogniser.rs
- confidence: inferred
  path: adapters/inference-deepgram/tests/claims_record.rs
- confidence: cited
  path: adapters/inference-deepgram/tests/conformance.rs
- confidence: inferred
  path: adapters/inference-deepgram/tests/fixtures
- confidence: cited
  path: adapters/inference-deepgram/tests/support/mod.rs
- confidence: inferred
  path: crates/inference-docs/src/adapters.rs
- confidence: inferred
  path: crates/inference-testkit/src/stt.rs
- confidence: inferred
  path: crates/inference-testkit/src/stt/conformance.rs
- confidence: inferred
  path: crates/inference-testkit/tests/stt_conformance.rs
- confidence: inferred
  path: website/data/status.json
- confidence: inferred
  path: website/docs/reference/adapters.md
- confidence: inferred
  path: website/docs/reference/crates.md
- confidence: inferred
  path: website/docs/reference/environment.md
- confidence: inferred
  path: website/docs/status.mdx
revision: 30
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T13:07:49Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-05T16:11:00Z", actor: "human:operator", revision: 29}
- {from: "active", to: "implemented", at: "2026-11-06T10:49:22Z", actor: "human:operator", revision: 30, decided_on: {"recorded":{"test_result":2,"review_outcome":4,"verification":1}}}
---
## Context

For a cascade on phone audio, a downstream adopter's survey names Deepgram as the strongest
speech-to-text layer: it is strong on 8 kHz audio, accepts native μ-law, has an EU endpoint, and
its Flux model detects turns itself. inference's STT adapters are local only: sherpa-onnx
(streaming, English) and whisper.cpp (batch).

## Planned scope

- A streaming STT binding of the `inference-stt` contract over Deepgram's live transcription
  WebSocket, in `adapters/inference-deepgram`:
  - partial and final transcripts;
  - the provider's end-of-turn signal, where one is given;
  - its claims record;
  - the protocol from public documentation, saved with its date.

## Acceptance

- The backend-neutral STT checks pass against a local fixture server.
- The live test is ignored and behind feature `live`. It needs a Deepgram API key with STT
  access.

## Scope

Derived 2026-11-05 by `story-scoper` at 4cc740b. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `adapters/inference-deepgram`, a new crate shared with
  story:deepgram-voice-agent-adapter — cited (both stories' Planned scope). This story lands first
  and creates the crate; the Voice Agent story `depends_on` it
- **Files:**
  - `adapters/inference-deepgram/Cargo.toml` — cited; the root workspace takes `adapters/*` by glob (`Cargo.toml:3`)
  - `adapters/inference-deepgram/src/lib.rs`, `src/config.rs` (EU host, model, `mulaw` at 8 kHz), `src/credential.rs`, `src/error.rs` — inferred, from the `inference-elevenlabs` layout
  - `adapters/inference-deepgram/src/listen.rs`, the `/v1/listen` protocol with its source pages and retrieval date in the module doc, as `adapters/inference-elevenlabs/src/protocol.rs:1-15` does — inferred. A module per endpoint, so the Voice Agent unit does not edit the same file
  - `adapters/inference-deepgram/src/recogniser.rs`, implementing `Recogniser` and `RecognitionHandle` (`crates/inference-stt/src/lib.rs:944,969`) — inferred
  - `adapters/inference-deepgram/records/claims-stt.json` — cited for the record, inferred for the name
  - `adapters/inference-deepgram/tests/support/mod.rs`, a local WebSocket fixture server modelled on `adapters/inference-elevenlabs/tests/support/mod.rs` — cited
  - `adapters/inference-deepgram/tests/claims_record.rs`, `tests/conformance.rs`, `tests/fixtures/` — cited for conformance, inferred for the rest
  - `adapters/inference-deepgram/examples/live.rs` behind feature `live` — cited for the live check, inferred for its form
  - `crates/inference-docs/src/adapters.rs` (a `Binding` entry), `Cargo.lock` — inferred, from commit 46a5289
- **Symbols:** `inference_stt::Recogniser`, `RecognitionHandle`, `TranscriptOutput::{hypothesis, revise, commit, usage}`, `TranscriptEvent` (`crates/inference-stt/src/lib.rs:260`) — cited
- **Backend-neutral STT checks do not exist yet** — cited: the testkit has only `tts::conformance` (`crates/inference-testkit/src/tts.rs:100`, added in e9a8243 with about 1,200 lines); `src/stt/` holds `fan_out.rs`, `node.rs`, `synthetic_audio.rs`; sherpa-onnx and whisper.cpp run adapter-specific checks against real checkpoints. This unit adds `crates/inference-testkit/src/stt/conformance.rs`, `pub mod conformance` in `crates/inference-testkit/src/stt.rs`, and `crates/inference-testkit/tests/stt_conformance.rs` running the mock recogniser through it — inferred
- **End-of-turn:** `inference-stt` has no type for it — cited (`TranscriptEvent` has Hypothesis, Revision, Committed and Usage, `lib.rs:260-269`; turn completion is `inference.stream.CompleteTurn`, `ess/domains/stream.yaml:441,954`, owned by the turn-policy node; the design keeps turn completion apart from span commit, `docs/design/inference.md:606`). Decided by the coordinator on 2026-11-05: the provider's end-of-turn signal maps onto `commit` of the open span, as the sherpa-onnx endpoint does (`adapters/inference-sherpa-onnx/src/stt.rs:22,809`). No new noun, no `ess/` change, no contract change
- **Also likely:** the generated `website/docs/reference/adapters.md`, `crates.md`, `website/docs/status.mdx`, `website/data/status.json`; `website/docs/reference/environment.md`; `CHANGELOG.md` — inferred
- **Confidence:** medium. The crate mirrors ElevenLabs closely; the STT conformance procedure is new work of the size of the TTS one
- **Would collide with:** story:deepgram-voice-agent-adapter on `Cargo.toml`, `src/lib.rs`, `config.rs`, `credential.rs`, `error.rs`, `tests/support/mod.rs`, `tests/claims_record.rs`; any unit on `crates/inference-docs/src/adapters.rs`, the generated pages, or `crates/inference-testkit/src/stt*`
- **Not established:** Deepgram's protocol details (Flux against Nova endpoints, message names, `mulaw` on `/v1/listen`); no Deepgram documentation was read at scoping
