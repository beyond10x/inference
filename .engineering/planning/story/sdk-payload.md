---
format: aep.planning-md/3
id: story:sdk-payload
kind: story
status: draft
title: One shared Payload type and defaulted port traits replace per-application payload enums
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-model-nodes
- depends_on: story:sdk-duplex-node
- depends_on: story:sdk-crate
scope:
- confidence: inferred
  path: crates/inference-pipeline/src/builder.rs
- confidence: inferred
  path: crates/inference/src/graph.rs
- confidence: inferred
  path: crates/inference/src/payload.rs
- confidence: inferred
  path: crates/inference/tests/graph.rs
- confidence: inferred
  path: examples/local-voice-cascade/Cargo.toml
- confidence: inferred
  path: examples/local-voice-cascade/src/lib.rs
revision: 15
---
## Context

G4 of `docs/design/sdk.md` §1 and §3.4: every application declares its own payload enum and
implements the workspace's payload traits on it (`examples/local-voice-cascade/src/lib.rs:182`
`VoicePayload`, seven single-line `impl … for VoicePayload` at `:201-338`, and `Handoff` at `:141`).
Serves O5; gives L3 (`inference::graph`, `inference::Payload`) its content.

The payload lives in the `inference` crate, not in `inference-pipeline` (decision D3 of
`api-design:sdk`): `SessionPayload` is in `inference-facade` (`crates/inference-facade/src/events.rs:124`),
which depends on the pipeline, and `PlaybackPayload` is in `inference-audio-device`
(`adapters/inference-audio-device/src/acknowledging.rs:136`) behind its `pipeline` feature, so a
payload in the pipeline could implement neither without a crate cycle.

## Domain relations

No new noun: the payload is the Rust carrier of the port modalities `inference.graph` declares.

## Planned scope

- `crates/inference/src/payload.rs`: `Payload`, one `#[derive(Debug, Clone)]` enum with a variant per
  modality the workspace's nodes speak. Three variants keep the shapes of `VoicePayload`
  (`examples/local-voice-cascade/src/lib.rs:181-193`): `Audio(AudioFrame)` (`inference_audio`),
  `Stream(StreamEvent)` (the type story:sdk-model-nodes moves to `inference_pipeline::nodes::log`,
  which the testkit re-exports as `inference_testkit::stream::StreamEvent`, so the two paths name one
  type) and `Control(ControlKind)` (`inference_core::envelope`), with `From<ControlKind>`. The
  speech handoff and the playback receipt (`VoicePayload::Speech`, `::Receipt`, over `Handoff`,
  `lib.rs:141`) move in as `Payload` variants of their own, beside the duplex, tool and session
  carriers. The duplex carrier includes `Payload::SpeechClosed(SpeechSessionClosed)`, built by
  `DuplexPayload::closed` (story:sdk-duplex-node), whose `session_event` is
  `SessionEvent::SpeechSessionClosed`, and `Payload::SpeechStarted(SpeechStarted)`, built by
  `DuplexPayload::speech_started`, whose `session_event` is `SessionEvent::SpeechStarted` (R134),
  and `Payload::ResponseFailed(ResponseFailed)`, built by `DuplexPayload::response_failed`, whose
  `session_event` is `SessionEvent::SpeechResponseFailed` (R143), and `Payload::SpeechFailed(SpeechSessionFailed)`, built by
  `DuplexPayload::failed`, whose `session_event` is `SessionEvent::SpeechSessionFailed` (R170).
  The enum is `#[non_exhaustive]`, because the `Receipt` variant
  exists only behind the feature `audio-host` and a downstream match must not depend on the
  feature set. The example's patterns over it still compile: every one outside the defining crate is
  a constructor, an `if let` (`examples/local-voice-cascade/examples/talk.rs:243`), a `let … else`
  (`talk.rs:258`, `tests/local_adapters.rs:511`, `:543`, `tests/gates.rs:314`, `:346`) or a
  `match` ending in `_ => None` (`tests/local_adapters.rs:589-592`, `tests/gates.rs:428-431`).
  It implements `AudioPayload` (`nodes/vad.rs:98`), `TurnPayload`
  (`nodes/turn_policy.rs:63`), `GatePayload` (`nodes/commit_gate.rs:92`), `ToolPayload`
  (`nodes/mod.rs:98`), and from story:sdk-model-nodes and story:sdk-duplex-node `SttPayload`,
  `ModelPayload`, `TtsPayload`, `SpeechPayload` and `DuplexPayload`; `SessionPayload` from the
  facade; `PlaybackPayload` behind the feature `audio-host`.
- The usage carrier (R98). Usage reaches a session today only through a payload variant that
  carries `UsageRecorded`: the testkit's `CascadePayload::Usage` (`crates/inference-testkit/src/cascade.rs:123`),
  whose `SttPayload::usage`, `ModelPayload::usage` and `TtsPayload::usage` return `Some(Self::Usage(..))`
  (`:201`, `:220`, `:243`) and whose `session_event` maps it to `SessionEvent::UsageRecorded` (`:190`).
  `VoicePayload` has no such variant (`examples/local-voice-cascade/src/lib.rs:181-193`), so its
  `usage` returns the trait default, `None` (`crates/inference-testkit/src/llm/node.rs:164`,
  `stt/node.rs:89`, `tts/node.rs:90`), and the example's usage never reaches its session. `Payload` gains
  `Usage(UsageRecorded)` with the same three `usage` bodies and the same `session_event` arm; `heard`
  maps it to `Heard::Other` and `carried` to `Carried::Usage`, as the testkit does (`:208`, `:229`).
  `Payload` is `#[non_exhaustive]`, so the example's matches keep compiling.
- `crates/inference/src/graph.rs`: `inference::graph`, the pipeline's typed builder with `Payload`
  fixed, re-exporting the declaration and planning types.
- `crates/inference-pipeline/src/builder.rs`: default `PortTraits` per modality, so a port
  declaration needs only node, name and modality.
- `examples/local-voice-cascade/src/lib.rs` and `Cargo.toml`: the enum `VoicePayload`, its seven `impl`s and
  `Handoff` removed; the lib keeps `pub type VoicePayload = inference::Payload;`, so every file that
  names `VoicePayload` compiles unchanged. `rg -l VoicePayload examples/ crates/ adapters/` lists
  `examples/local-voice-cascade/src/lib.rs`, `examples/talk.rs` and `tests/{local_adapters,gates,
  barge_in,adversary1_barge_in_timing}.rs`; they use only `VoicePayload::Audio`, `::Stream` and
  `::Control`, as constructors and in patterns that end in `_ => None` or a `let … else`
  (`tests/gates.rs:428-431`, `tests/local_adapters.rs:589-592`), never `Speech` or `Receipt`, and
  none names `Handoff`. The example gains the `inference` dependency.
- `crates/inference/tests/graph.rs` (new).

## Acceptance

- `cargo test -p local-voice-cascade` exits 0 with the enum `VoicePayload` and `Handoff` deleted and
  `pub type VoicePayload = inference::Payload;` in their place, `git diff` empty for its five
  `tests/*.rs` and `examples/talk.rs`.
- `rg -l VoicePayload examples/ crates/ adapters/` lists only files under
  `examples/local-voice-cascade/`, which this story (`src/lib.rs`) and story:sdk-examples (the
  directory) scope; `rg -n "enum VoicePayload|struct Handoff" examples/` exits 1.
- `rg -n -B2 'pub enum Payload' crates/inference/src/payload.rs` shows `#[non_exhaustive]`, and
  `cargo test -p local-voice-cascade` (above) compiles the example's patterns against it, with and
  without `--features local`.
- A unit test in `payload.rs`: `Payload::closed(SpeechSessionClosed { .. }).session_event()` is
  `Some(SessionEvent::SpeechSessionClosed(..))` with the same `session_id`, and
  `Payload::speech_started(SpeechStarted { .. }).session_event()` is
  `Some(SessionEvent::SpeechStarted(..))` with the same `position`, and
  `Payload::response_failed(ResponseFailed { .. }).session_event()` is
  `Some(SessionEvent::SpeechResponseFailed(..))` with the same `response_id`, and
  `Payload::failed(SpeechSessionFailed { .. }).session_event()` is
  `Some(SessionEvent::SpeechSessionFailed(..))` with the same `session_id`.
- Usage reaches the session (R98). In `crates/inference/tests/graph.rs`, a pipeline declared through
  `inference::graph`: an exposed text ingress, the testkit model node
  `ModelNode::carrying(..).with_usage(<its usage port>)` over `inference::Payload`, its `usage` port
  (`inference_pipeline::USAGE_PORT`) exposed as the testkit cascade exposes `llm_usage`
  (`crates/inference-testkit/src/cascade.rs:101`, `:479`). Opened as a `LiveSession<Payload>`, after one
  turn `session.usage()` holds one record whose token counts are the mock generator's. A unit test in
  `payload.rs`: `<Payload as ModelPayload>::usage(r)` is `Some(Payload::Usage(r))` and its
  `session_event()` is `Some(SessionEvent::UsageRecorded(r))`.
- `grep -rn 'impl .*Payload for' examples/*/src` exits 1. Today it finds 7 lines, all in
  `examples/local-voice-cascade/src/lib.rs`.
- A doc test on `inference::graph` declares, validates and plans the reference cascade through
  `inference::graph` and `inference::Payload` alone.
- `crates/inference/tests/graph.rs` declares the reference cascade's ports twice, once with the
  explicit `PortTraits` that `local_voice_cascade::declare` (`examples/local-voice-cascade/src/lib.rs:458`)
  writes today and once with the defaulted traits, and the two planned pipelines are equal (same
  nodes, ports, edges and traits).
- `cargo test -p inference --test graph` passes (the port-traits test), and so does
  `cargo test -p inference --doc` (the doc test on `inference::graph`).

Its diff lists none of `crates/inference/src/lib.rs`, `crates/inference/src/prelude.rs` and
`crates/inference/Cargo.toml` (story:sdk-crate wrote them once, decision D9 of `api-design:sdk`).

## Out of scope

Payloads for modalities no node speaks yet (images, video).
