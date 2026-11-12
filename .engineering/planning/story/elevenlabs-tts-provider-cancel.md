---
format: aep.planning-md/3
id: story:elevenlabs-tts-provider-cancel
kind: story
status: implemented
title: ElevenLabs TTS cancel drops a segment on the client, and time to first audio is readable
summary: A cancel stops a segment's playback and drops its remaining frames; the hosted endpoint does not stop a flushed context on close (measured); each segment's time to first audio reaches the caller.
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
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-12T09:37:26Z", actor: "human:operator", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-12T09:37:26Z", actor: "human:operator", revision: 6}
- {from: "active", to: "implemented", at: "2026-11-12T13:27:54Z", actor: "human:operator", revision: 9, decided_on: {"recorded":{"test_result":2,"review_outcome":3}}, correlation: "wave-elevenlabs-route-p1"}
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

Revised on 2026-11-12, after a billed probe of the hosted `multi-stream-input` endpoint
(test_result evidence on this story; `adapters/inference-elevenlabs/tests/tts_cancel_live.rs`):
a `close_context` sent 138 ms after the first audio did not stop generation (99% of the text's
audio arrived after it), the whole 22.7 s of audio was generated within 785 ms, and a flush
without a close got its final message only 20012 ms after the last audio. A provider-side cancel
is therefore not available on this endpoint; the close stays with the text, and a cancel stops
playback on the client. The first acceptance line below replaces "the fixture server receives the
context close for that context" (review-result:security-elevenlabs-tts-cancel-pass-1, finding 3).

- A fixture test: after a cancel, the caller receives no frame of that context, and no second
  context close is sent.
- A fixture test: the next segment speaks on the same connection after a cancel.
- A fixture test: time to first audio is present for every spoken segment and is at least the
  fixture server's injected delay.
- The crate doc states the measured behaviour of a close on the hosted endpoint, with its date.
- A billed live probe of the close and flush behaviour exists, ignored, behind feature `live`.
