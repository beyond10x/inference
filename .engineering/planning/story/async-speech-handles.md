---
format: aep.planning-md/3
id: story:async-speech-handles
kind: story
status: draft
title: Async handles for synthesis, recognition and speech sessions
summary: A Stream or async next_event on each speech handle, so a tokio service drives calls without a thread each.
tags:
- downstream-input
relations:
- decomposes: epic:elevenlabs-route
- serves: vision:inference
- depends_on: story:elevenlabs-realtime-stt-adapter
- depends_on: story:elevenlabs-tts-provider-cancel
- depends_on: story:elevenlabs-agent-session-overrides
scope:
- confidence: cited
  path: adapters
- confidence: cited
  path: crates/inference-duplex
- confidence: cited
  path: crates/inference-pipeline
- confidence: cited
  path: crates/inference-stt
- confidence: cited
  path: crates/inference-testkit
- confidence: cited
  path: crates/inference-tts
revision: 3
---
## Context

`SynthesisHandle` (`crates/inference-tts/src/lib.rs:1271`) and `RecognitionHandle`
(`crates/inference-stt/src/lib.rs:1031`) are `Iterator`s, and `SpeechSessionHandle`
(`crates/inference-duplex/src/contract.rs:361`) is polled the same way. A tokio service, such as
a phone voice-agent consumer, runs one blocked thread per call to drive them.

## Planned scope

- An async form of each handle: a `futures::Stream` of its events, or an async `next_event`,
  woken by the adapter when an event arrives rather than polled; the blocking forms may stay.
- Every adapter implementing the three contracts, the ElevenLabs recogniser, synthesiser and agent
  among them, and the testkit mocks provide it; the pipeline nodes use it where they wait today.
- The contract crates take no runtime dependency beyond `futures-core`; adapters that already run
  tokio wake the stream from their socket task.
- A contract change is declared in `ess/` first where it changes an event or a command, validated
  by `ess specify validate --path ess`.
- Worked alone: its surface is the three contract crates, every speech adapter, the testkit and
  `crates/inference-pipeline`, so no other story of the set is worked beside it. It depends on
  story:elevenlabs-realtime-stt-adapter, story:elevenlabs-tts-provider-cancel and
  story:elevenlabs-agent-session-overrides, which create or rewrite the ElevenLabs bindings it
  converts and share `crates/inference-tts/src/lib.rs` and `agent_duplex.rs` with it;
  story:duplex-session-config-additions depends on it through task:release-elevenlabs-route-p2
  and shares `crates/inference-duplex/src/contract.rs`.

## Acceptance

- One test drives 20 concurrent fixture sessions of each contract (the ElevenLabs recogniser,
  synthesiser and agent against their fixture servers) on a current-thread tokio runtime without
  `spawn_blocking`, and every session delivers all its events.
- The backend-neutral checks of the three contracts pass through the async form.
- `cargo tree -p inference-tts -p inference-stt -p inference-duplex -e normal` shows no `tokio`.
