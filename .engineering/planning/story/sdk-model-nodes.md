---
format: aep.planning-md/3
id: story:sdk-model-nodes
kind: story
status: draft
title: The STT, LLM, agent, TTS and speech nodes live in inference-pipeline, not the testkit
relations:
- decomposes: epic:sdk
- serves: vision:inference
scope:
- confidence: inferred
  path: crates/inference-pipeline/Cargo.toml
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/agent.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/llm.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/log.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/mod.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/speech.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/stt.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/tts.rs
- confidence: inferred
  path: crates/inference-testkit/src/cascade.rs
- confidence: inferred
  path: crates/inference-testkit/src/cascade/agent.rs
- confidence: inferred
  path: crates/inference-testkit/src/cascade/speech.rs
- confidence: inferred
  path: crates/inference-testkit/src/graph/nodes.rs
- confidence: inferred
  path: crates/inference-testkit/src/llm.rs
- confidence: inferred
  path: crates/inference-testkit/src/llm/node.rs
- confidence: inferred
  path: crates/inference-testkit/src/stream.rs
- confidence: inferred
  path: crates/inference-testkit/src/stt.rs
- confidence: inferred
  path: crates/inference-testkit/src/stt/node.rs
- confidence: inferred
  path: crates/inference-testkit/src/tts.rs
- confidence: inferred
  path: crates/inference-testkit/src/tts/node.rs
- confidence: inferred
  path: examples/local-voice-cascade/src/lib.rs
revision: 8
---
## Context

G1 of `docs/design/sdk.md` §1: the STT, LLM, agent, TTS and speech graph nodes exist only in
`inference-testkit` (`crates/inference-testkit/src/stt/node.rs` `SttNode` :124, `tts/node.rs`
`TtsNode` :114, `llm/node.rs` `ModelNode` :279, `cascade/agent.rs` `AgentNode` :118,
`cascade/speech.rs` `SpeechNode` :139), so a production cascade imports its nodes from the testkit
(`examples/local-voice-cascade/src/lib.rs:108-117`). Serves O5.

The five files are not self-contained. Each imports from the testkit `crate::graph::nodes::{Emitter,
Exchange, PipelinePayload, RunLog, Watched, block_on, send_unless}` and `crate::stream::StreamEvent`
(for example `stt/node.rs:62-66`). `StreamEvent` (`stream.rs:212`) and `EventLog` sit in the
1965-line in-memory stream domain, whose `inference_runtime` import is behind the `run` feature
(`stream.rs:78-79`); `RunLog` and `PipelinePayload` are in `graph/nodes.rs:58` and `:38`.
`inference-pipeline` has crate-private equivalents of four helpers (`nodes/mod.rs:115` `Watched`,
`:209` `block_on`, `:293` `Emitter`; `nodes/tool_authority.rs:119` `send_unless`).
`inference-runtime` depends on `inference-pipeline` (`crates/inference-runtime/Cargo.toml:12`), so
nothing moved into the pipeline may need the runtime.

## Domain relations

No new noun: the nodes are `inference.graph.Node` implementations of the roles `model` and `sink`
already declared in `ess/domains/graph.yaml`.

## Planned scope

- `crates/inference-pipeline/src/nodes/`: `stt.rs`, `llm.rs`, `agent.rs`, `tts.rs`, `speech.rs`
  holding `SttNode`, `ModelNode`, `AgentNode`, `TtsNode`, `SpeechNode`, their handles and their
  payload traits (`SttPayload` `stt/node.rs:83`, `ModelPayload` `llm/node.rs:158`, `TtsPayload`
  `tts/node.rs:81`, `SpeechPayload` `cascade/speech.rs:93`), beside `vad.rs`, `commit_gate.rs` and
  `tool_executor.rs` (Q2 default: `inference-pipeline::nodes`).
- `crates/inference-pipeline/src/nodes/log.rs` (new): `StreamEvent`, `EventLog`, `EnvelopeRefusal`
  and `RunLog`, moved without the stores of `stream.rs`. `Exchange` moves into `nodes/mod.rs`; the
  other helpers use the pipeline's own.
- The default type parameters that name testkit types (`SttNode<R = MockRecogniser>`,
  `TtsNode<S = MockSynthesiser>`, `ModelNode<P = PipelinePayload>`) leave the moved types; the
  testkit re-exports each under its old path as a type alias with the old default.
- `crates/inference-pipeline/Cargo.toml`: `inference-stt`, `inference-llm` (feature `tools`) and
  `inference-tts` as normal dependencies; none of them depends on the pipeline.
- Testkit re-exports: `src/stt.rs`, `src/stt/node.rs`, `src/tts.rs`, `src/tts/node.rs`,
  `src/llm.rs`, `src/llm/node.rs`, `src/cascade.rs`, `src/cascade/agent.rs`,
  `src/cascade/speech.rs`, `src/stream.rs`, `src/graph/nodes.rs`.
- `examples/local-voice-cascade/src/lib.rs`: node and payload-trait imports from
  `inference_pipeline::nodes`. Its mocks stay imported from the testkit, which stays a normal
  dependency of the example until story:sdk-examples.

## Acceptance

- `cargo test --workspace --all-features` exits 0, and `git diff --name-only` of the story lists no
  file under a `tests/` directory: the re-exports keep every test unchanged.
- `rg -U -n 'use inference_testkit::[^;]*(SttNode|AgentNode|SpeechNode|ModelNode|TtsNode|SttPayload|ModelPayload|SpeechPayload|TtsPayload)' examples/local-voice-cascade/src`
  exits 1. Today it matches at `lib.rs:108`, `:109-110`, `:114` and `:117`.
- `cargo tree -p inference-pipeline -e normal` lists neither `inference-testkit` nor
  `inference-runtime`.
- Conformance baseline: `cargo test -p inference-testkit --features run --test conform` passes with
  the sixteen arrays present at the story's start (456 ids on main at `dc8e13e`) all still passing
  and `ess/SKIPPED.md` listing 0 ids; `SDK_SCENARIOS` (story:sdk-stack, story:sdk-recipes) is not
  counted. The count goes by array name, not by line range, so arrays other stories add in the
  same file do not shift it:
  `rg -o -r '$1 $2' 'const ([A-Z_]+_SCENARIOS): \[&str; ([0-9]+)\]' crates/inference-testkit/tests/conform.rs | awk '$1 != "SDK_SCENARIOS" { n++; s += $2 } END { print n, s }'`
  prints `16 456` (counted 2026-10-22). Of these, 450 are synthesized ids
  (`rg -o '"inference\.[a-z_]+\.[A-Za-z]+/[^"]*"'` → 450) and 6 are authored ids of the form
  `inference.<domain>/authored/<name>` (graph 1, stream 3, duplex 2), which the synthesized-only
  pattern misses: that is where the earlier 450 came from. `counts.total` of the run's `report.json`
  is read at implementation start, before the move (no recent record in `.engineering/evidence/`),
  and is the same after.

Its diff lists none of `crates/inference/src/lib.rs`, `crates/inference/src/prelude.rs` and
`crates/inference/Cargo.toml` (story:sdk-crate wrote them once, decision D9 of `api-design:sdk`).

## Out of scope

Changing any node's behaviour; the duplex node (story:sdk-duplex-node); the stream stores, which stay
in the testkit.
