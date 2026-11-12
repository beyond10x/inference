---
format: aep.planning-md/3
id: story:async-speech-handles
kind: story
status: active
title: Async handles for synthesis, recognition and speech sessions
summary: A Stream or async next_event on each speech handle, so a tokio service drives calls without a thread each.
tags:
- downstream-input
relations:
- decomposes: epic:elevenlabs-route
- serves: vision:inference
- depends_on: story:elevenlabs-realtime-stt-adapter
- depends_on: story:elevenlabs-tts-provider-cancel
- depends_on: story:realtime-voice-barge-in-stop-flake
scope:
- confidence: inferred
  path: Cargo.lock
- confidence: cited
  path: adapters/inference-aws/src/duplex.rs
- confidence: cited
  path: adapters/inference-cartesia/src/synthesiser.rs
- confidence: cited
  path: adapters/inference-deepgram/src/agent_duplex.rs
- confidence: cited
  path: adapters/inference-deepgram/src/recogniser.rs
- confidence: cited
  path: adapters/inference-deepslate/src/opal.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/agent_duplex.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/recogniser.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/synthesiser.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/tests
- confidence: cited
  path: adapters/inference-openai/src/duplex.rs
- confidence: cited
  path: adapters/inference-openai/src/live_duplex.rs
- confidence: cited
  path: adapters/inference-openai/src/speech/mod.rs
- confidence: cited
  path: adapters/inference-phonic/src/duplex.rs
- confidence: cited
  path: adapters/inference-sherpa-onnx/src/stt.rs
- confidence: cited
  path: adapters/inference-whispercpp/src/stt.rs
- confidence: cited
  path: crates/inference-duplex
- confidence: inferred
  path: crates/inference-duplex/Cargo.toml
- confidence: cited
  path: crates/inference-duplex/src/contract.rs
- confidence: cited
  path: crates/inference-stt
- confidence: inferred
  path: crates/inference-stt/Cargo.toml
- confidence: cited
  path: crates/inference-stt/src/lib.rs
- confidence: cited
  path: crates/inference-testkit
- confidence: cited
  path: crates/inference-testkit/src/cascade/late.rs
- confidence: cited
  path: crates/inference-testkit/src/duplex/mock.rs
- confidence: cited
  path: crates/inference-testkit/src/stt.rs
- confidence: cited
  path: crates/inference-testkit/src/tts.rs
- confidence: cited
  path: crates/inference-tts
- confidence: inferred
  path: crates/inference-tts/Cargo.toml
- confidence: cited
  path: crates/inference-tts/src/lib.rs
revision: 46
transitions:
- {from: "draft", to: "proposed", at: "2026-11-12T15:36:19Z", actor: "human:operator", revision: 45, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "proposed", to: "active", at: "2026-11-12T15:36:19Z", actor: "human:operator", revision: 46, decided_on: {"recorded":{"review_outcome":2}}}
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
  among them, and the testkit mocks provide it. `crates/inference-pipeline` keeps its blocking
  waits in this story.
- The contract crates take no runtime dependency beyond `futures-core`; adapters that already run
  tokio wake the stream from their socket task.
- A contract change is declared in `ess/` first where it changes an event or a command, validated
  by `ess specify validate --path ess`.
- Order: it is worked last in the wave, after story:realtime-voice-barge-in-stop-flake and so
  after story:compare-elevenlabs-cascade-and-ollama (a `depends_on` edge). `examples/realtime-voice`,
  which those two change, builds against the contract crates and the ElevenLabs recogniser this
  story converts. story:status-page-voice-adapters shares no file with it. It depends on the
  implemented P1 stories story:elevenlabs-realtime-stt-adapter and
  story:elevenlabs-tts-provider-cancel, which wrote the ElevenLabs bindings it converts.
  story:duplex-session-config-additions, outside this wave, depends on it through
  task:release-elevenlabs-route-p2 and shares `crates/inference-duplex/src/contract.rs`.

## Acceptance

- A test in `crates/inference-testkit/tests/` drives 20 concurrent testkit mock sessions of each
  contract (TTS, STT, duplex) through the async form on a current-thread tokio runtime without
  `spawn_blocking`. A ticker task on the same runtime ticks every 10 ms while the sessions wait.
  The test passes only if every session delivers all its events and the ticker's largest gap
  stays under 50 ms, so a default form that blocks the runtime thread fails it.
- One test per ElevenLabs handle (recogniser, synthesiser, agent) under
  `adapters/inference-elevenlabs/tests/` does the same against its fixture server, with 20
  concurrent sessions and the same ticker bound.
- `inference_testkit::tts::conformance`, `inference_testkit::stt::conformance` and
  `inference_duplex::conformance` each run once more through the async form, driving the testkit
  mocks on a current-thread runtime, in one test per contract under
  `crates/inference-testkit/tests/`, and pass.
- `cargo tree -p inference-tts -p inference-stt -p inference-duplex -e normal` shows no `tokio`.

## Design decided at scoping (2026-11-12)

- The async form has a default on each of the three handle traits, built on the blocking form, and
  the adapters named in the scope override it with a non-blocking implementation. No test-side
  implementor of the traits has to change, and the blocking `Iterator` and `next_event(Duration)`
  forms stay.
- `examples/realtime-voice` and `examples/local-voice-cascade` keep their blocking calls in this
  story; moving them onto the async form is later work.
- The acceptance test of 20 concurrent sessions per contract lives in
  `crates/inference-testkit/tests/` against the testkit mocks, plus one per ElevenLabs handle
  under `adapters/inference-elevenlabs/tests/` against the fixture servers.
