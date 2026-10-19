---
format: aep.planning-md/3
id: story:voice-cascade-on-mocks
kind: story
status: active
title: 'The voice cascade on the testkit mocks: STT and TTS nodes and one cascade declaration with exposed ends'
summary: Graph nodes over the mock recogniser and synthesiser, and a testkit fixture declaring topology B with an audio ingress and transcript, text and audio egresses; a runtime test runs it offline.
relations:
- decomposes: epic:session-facade
- serves: vision:inference
- depends_on: story:pipeline-exposed-ports
scope:
- confidence: cited
  path: crates/inference-runtime/Cargo.toml
- confidence: cited
  path: crates/inference-runtime/tests/voice_cascade_mocks.rs
- confidence: inferred
  path: crates/inference-testkit/Cargo.toml
- confidence: cited
  path: crates/inference-testkit/src/cascade.rs
- confidence: cited
  path: crates/inference-testkit/src/lib.rs
- confidence: cited
  path: crates/inference-testkit/src/stt.rs
- confidence: cited
  path: crates/inference-testkit/src/stt/node.rs
- confidence: cited
  path: crates/inference-testkit/src/tts.rs
- confidence: cited
  path: crates/inference-testkit/src/tts/node.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:56:34Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-19T22:56:34Z", actor: "human:timo", revision: 5}
---
## Context

A-12 (`docs/design/inference.md:1148`) and the P4 exit (1162) need a voice cascade to switch to:
topology B (759-778) runs audio through VAD and endpointing, streaming STT, the text LLM, streaming
TTS and playback. On mocks most of it exists: the VAD and turn-policy nodes
(`crates/inference-pipeline/src/nodes/vad.rs`, `turn_policy.rs`, `story:endpoint-controller`), the
model node over the mock LLM (`crates/inference-testkit/src/llm/node.rs`), the mock recogniser and
its synthetic audio (`crates/inference-testkit/src/stt.rs`, `src/stt/synthetic_audio.rs`) and the
mock synthesiser (`crates/inference-testkit/src/tts.rs`). What does not exist is a graph node over
the recogniser or the synthesiser: the only `NodeImplementation`s in the tree are the text source
and sink, the recording node, the model node and the four pipeline nodes. So no cascade runs today,
on mocks or otherwise.

The two nodes wrap testkit mocks and live in the testkit, as the model node wraps the mock LLM
there. They are not the cascade of `epic:local-voice-cascade`: its commit-gate node, acknowledging
audio sink and real adapters stay that epic's.

Decisions taken on silence:

- The STT node takes audio frames on its input and emits the recogniser's transcript events, the
  committed spans also on an output named `committed_text`, the port name of the design's example
  (579).
- The TTS node takes committed text segments and emits audio chunks.
- The run's payload is one testkit type carrying audio frames, `inference.stream` events and
  control events, implementing the payload traits the VAD, turn-policy and model nodes require, as
  each runtime test defines its own today (`crates/inference-runtime/tests/endpoint_controller.rs:75-105`).
- The fixture declares the cascade with exposed ends (`story:pipeline-exposed-ports`): an audio
  ingress at the VAD, and egresses for the transcript events, the committed assistant text and the
  synthesized audio. It takes the model node's start rule as it is; the turn-gated start belongs
  to `story:cascade-read-only-tool`, which edits that node.
- No new dependency: the runtime test reaches the mocks through the testkit features `stt`, `llm`
  and `tts`, so `Cargo.lock` does not change.

## Planned scope

- `crates/inference-testkit/src/stt/node.rs` (new) with its `mod` line in `src/stt.rs`;
  `crates/inference-testkit/src/tts/node.rs` (new) with its `mod` line in `src/tts.rs`.
- `crates/inference-testkit/src/cascade.rs` (new) with its `mod` line in `src/lib.rs`: the payload
  type, and a function that declares, validates and plans the cascade over a `Declaration` and
  returns the node implementations for one run.
- `crates/inference-runtime/tests/voice_cascade_mocks.rs` (new); `crates/inference-runtime/Cargo.toml`
  adds feature `tts` to its testkit dev-dependency.

## Domain relations

- Pipeline owns Node and Edge, many: `ess/domains/graph.yaml`, `inference.graph.Pipeline`,
  relations `nodes`, `edges`; and its exposures, declared by `story:pipeline-exposed-ports`.
- Span references Turn and Run, one each: `ess/domains/stream.yaml`, `inference.stream.Span`,
  relations `turn`, `run`.
- Generation owns OutputSegment, many, and references the Node producing it, one:
  `ess/domains/stream.yaml`, `inference.stream.Generation`, relations `segments`, `producer`.

No noun is added and `ess/` does not change.

## Acceptance

`crates/inference-runtime/tests/voice_cascade_mocks.rs`, with no network, feeds the synthetic
utterance into the cascade's audio ingress and reads from its egresses a committed transcript span
of the scripted question, committed assistant text of the scripted reply and the audio chunks the
mock synthesiser produced for that text, and sees the run reach Completed after `stop` with every
queue dropped.

## Out of scope

The commit-gate node, the real adapters, the acknowledging audio sink and the A-04 network test
(`epic:local-voice-cascade`); tools in the cascade (`story:cascade-read-only-tool`); barge-in; a
session over the cascade (`story:session-topology-switch`); node descriptors for the two nodes.
