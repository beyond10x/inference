---
format: aep.planning-md/3
id: story:elevenlabs-tts-provider-cancel
kind: story
status: active
title: ElevenLabs TTS cancel stops generation at the provider, and time to first audio is readable
summary: A cancel closes the segment's context so the provider stops generating; each segment's time to first audio reaches the caller.
tags:
- downstream-input
relations:
- decomposes: epic:elevenlabs-route
- serves: vision:inference
- depends_on: story:elevenlabs-realtime-stt-adapter
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: adapters/inference-elevenlabs/Cargo.toml
- confidence: inferred
  path: adapters/inference-elevenlabs/src/connection.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/error.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/lib.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/src/protocol.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/synthesiser.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests
- confidence: cited
  path: crates/inference-tts/src/lib.rs
- confidence: inferred
  path: ess/domains/audio.yaml
- confidence: inferred
  path: ess/domains/stream.yaml
- confidence: inferred
  path: examples/realtime-voice
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-12T09:37:26Z", actor: "human:operator", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-12T09:37:26Z", actor: "human:operator", revision: 6}
---
## Context

The ElevenLabs synthesiser flushes and closes each segment's context when it sends the text, so a
cancel stops nothing at the provider: "It saves playback, not cost"
(`adapters/inference-elevenlabs/src/lib.rs:26-31`). On a phone call every barge-in then pays for
the whole cut segment. A phone voice-agent consumer also needs the time to first audio of each
segment, which the synthesis events do not carry.

## Planned scope

- `adapters/inference-elevenlabs/src/synthesiser.rs` and its connection: keep a segment's context
  open on the `multi-stream-input` connection until its audio is final, and on a cancel send the
  documented context close for that context so the provider stops generating; frames of the
  cancelled context that still arrive are dropped. The protocol is read again from ElevenLabs'
  documentation and the saved summary updated with its date.
- Time to first audio per segment, from the segment's offer to its first frame, readable by the
  caller: on the synthesis's events or output (`crates/inference-tts/src/lib.rs:218`,
  `SegmentAudio`, or `SynthesisOutput`) and in the latency report of
  story:voice-topology-latency-per-step. A contract change goes into `ess/domains/audio.yaml` or
  `stream.yaml` first, validated by `ess specify validate --path ess`, and every caller in the
  repository is updated in the same change.
- The crate doc at `lib.rs:26-31` states what a cancel now stops.
- Shared files: `adapters/inference-elevenlabs/src/lib.rs`, `src/error.rs`, `Cargo.toml` and
  `tests/claims_record.rs` with story:elevenlabs-realtime-stt-adapter, on which this story
  depends; `crates/inference-tts/src/lib.rs` with story:async-speech-handles, which depends on
  this story.

## Acceptance

- A fixture test: after a cancel, the fixture server receives the context close for that context
  and the caller receives no frame of it.
- A fixture test: the next segment speaks on the same connection after a cancel.
- A fixture test: time to first audio is present for every spoken segment and is at least the
  fixture server's injected delay.
