---
format: aep.planning-md/3
id: story:llm-contract
kind: story
status: draft
title: 'LLM contract with a mock generator: streaming and input-during-output verified apart'
summary: inference-llm is the generation contract over Generation and OutputSegment; the testkit mock generator declares output_streaming apart from concurrent_input_output and §23.1 test 2 verifies the two independently.
relations:
- decomposes: epic:contracts-and-testkit
- depends_on: story:outbound-text-generations-and-segments
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-llm
- confidence: cited
  path: crates/inference-testkit/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/src/llm.rs
- confidence: inferred
  path: crates/inference-testkit/tests/llm.rs
revision: 7
---
## Context

`docs/design/inference.md:364-376`, the LLM contract: input is text or structured context with generation instructions, output is text deltas or complete text, structured tool proposals, a structured result, status and usage (lines 366-367); output streaming, incremental input and input-during-output are distinct capabilities (line 372); generation ids and cancellation status survive tool round trips (line 376). The crate is `inference-llm` (line 195; line 237: depends on `core`, `text`, optionally `tools`). The contract projects the declared nouns (answer to Q2 in `executable-system-specification:inference`): an implementation is an `inference.model.Adapter` declaring `output_streaming`, `incremental_input` and `concurrent_input_output` separately (`inference.model.Capability`, lines 419-427), and its output is the `inference.stream` outbound commands `StartGeneration`, `EmitTextDelta`, `CommitSegment` and `CompleteGeneration`, with `CancelGeneration` and `SupersedeGeneration` as control.

A persistent connection is not proof of a streaming model (lines 43-54). §23.1 test 2 (line 1106): output streaming and input-during-output are independently verified, so one test cannot stand in for the other. The mock yields its events to the caller (an iterator or a callback) and records nothing in the testkit log, so this story touches no module of the inbound-text story and can run beside it. With `story:audio-and-stt-contract` and `story:tts-contract` it shares two lines of `crates/inference-testkit/Cargo.toml`, each story's own optional dependency and its own feature line, pre-declared empty by `story:testkit-conformance-target`, and nothing else: each fills its own feature-gated module, so the three may merge in any order, and `story:pipeline-declaration`, which also edits that file, runs after this story by its `depends_on` edge.

## Domain relations

- The mock generator is an `inference.model.Adapter` referencing one `inference.model.Model`: inferable, `ess/domains/model.yaml`, entity `inference.model.Adapter`, relation `model`.
- Its output is an `inference.stream.Generation` owning `inference.stream.OutputSegment`s and referencing a run and a producer node: inferable, `ess/domains/stream.yaml`, entity `inference.stream.Generation`, relations `run`, `producer` and `segments`. The mock is handed the `RunId` and `NodeId` it generates under; no Node exists in this epic.

## Steps

1. `crates/inference-llm/`: the generation contract: declare the adapter record; start a generation for a prompt (a committed text request); receive deltas as `OutputSegment`s in Tentative, then committed; complete, cancel or supersede through the `Generation` transitions of `inference-core`; an offer of further input while a generation is Active is accepted only by an adapter claiming `concurrent_input_output` and refused otherwise with a typed error naming the missing capability.
2. `inference-testkit`, feature `llm`: `crates/inference-testkit/Cargo.toml` gains the optional dependency on `inference-llm` and `llm = ["dep:inference-llm"]` in place of the `llm = []` the testkit story declared; the `llm` module, declared empty behind `#[cfg(feature = "llm")]` by that story, gains a scripted mock generator in three configurations claiming (a) `output_streaming` only, (b) neither, (c) `output_streaming` and `concurrent_input_output`; (a) emits deltas before completion, (b) emits one segment at completion, (c) accepts input mid-generation and reflects it in a later delta.
3. §23.1 test 2 as two separate Rust tests: `output_streaming` is verified by at least one `TextDelta` observed before `GenerationCompleted` for (a) and none for (b); `concurrent_input_output` is verified by an input offered while Active being accepted for (c) and refused for (a), with (a) still completing its generation. A third test asserts the declared capability set of each configuration equals the set the first two observed.

## Acceptance

`cargo test -p inference-testkit --features llm` passes with the three §23.1 test 2 tests: deltas before completion only where `output_streaming` is declared, input during an Active generation accepted only where `concurrent_input_output` is declared, and each mock configuration observed to claim exactly what it does.

## Out of scope

Tool proposals (line 374) and `inference-tools` (`epic:tools-and-turns`); the context semantics of stateless request, adapter-managed history or opaque model session (line 371) and token accounting (line 373), which `ess/` does not declare and the exit of the epic does not need, so `epic:local-llm-adapter` extends the specification for them; constrained decoding, structured output and embeddings (line 375); a real backend.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). No code exists yet (`README.md`, `AGENTS.md`; `git ls-files`
is empty), so every `crates/` path below is one the plan declares, not one on disk.

- **Primary surface:** `crates/inference-llm` — cited, a new crate and the whole of step 1 (`docs/design/inference.md:195`, `:237`: depends on `core`, `text`, optionally `tools`)
- **Files:** `crates/inference-testkit/Cargo.toml` — cited, two lines only: the optional dependency on `inference-llm` and `llm = ["dep:inference-llm"]` in place of `llm = []` (step 2; both slots pre-declared by `story:testkit-conformance-target` step 1)
- **Files:** `crates/inference-testkit/src/llm.rs` — inferred: the module `llm`, declared empty behind `#[cfg(feature = "llm")]`, is cited (step 2; testkit story step 1), but whether it is `llm.rs` or `llm/mod.rs` is the testkit story's choice and not yet on disk
- **Files:** `crates/inference-testkit/tests/llm.rs` — inferred: step 3's three tests run under `cargo test -p inference-testkit --features llm`; the story does not say whether they are an integration test file or `#[cfg(test)]` inside the module; a new file is assumed because `tests/conform.rs` and `tests/target/` belong to the testkit story
- **Symbols:** `inference.model.Adapter` (`ess/domains/model.yaml:100`, relation `model` `:124`), `inference.model.Capability` with `output_streaming`, `incremental_input`, `concurrent_input_output` (`:28-33`); `inference.stream.Generation` with relations `run`, `producer`, `segments` (`ess/domains/stream.yaml:121-148`), `inference.stream.OutputSegment` (`:172`), `StartGeneration`, `CompleteGeneration`, `SupersedeGeneration`, `CancelGeneration`, `EmitTextDelta`, `CommitSegment` (`:366-486`), `GenerationCompleted` (`:629`), `TextDelta` (`:644`) — cited; all consumed, none changed; their Rust homes `inference-core` and `inference-text` are not edited by this story
- **Not touched, by the bodies' own text:** root `Cargo.toml` (`members = ["crates/*"]`, `story:workspace-skeleton` step 1), `crates/inference-testkit/src/lib.rs` (the `mod llm` line is pre-declared so that "no later story adds a line to `lib.rs`", `story:testkit-conformance-target` step 1), `crates/inference-core`, `crates/inference-text`, `ess/`, `ess/SKIPPED.md` (the mock "records nothing in the testkit log"; acceptance is `--features llm`, not `--test conform`), `AGENTS.md`, `Taskfile.yml`, `README.md` — cited
- **Documents:** none — cited; Out of scope sends every specification extension (context semantics, token accounting) to `epic:local-llm-adapter`
- **Confidence:** high — the story names its crate directory and the two testkit slots it fills; only two file names inside the testkit are read rather than given
- **Would collide with:** any unit editing `crates/inference-testkit/Cargo.toml` (shared by design with the STT and TTS contract surfaces and the pipeline-declaration surface, each on its own lines); any unit creating `crates/inference-llm`; any unit changing the `Generation` transitions in `inference-core` or `OutputSegment` in `inference-text` — an API break, not a textual conflict
- **Safety fact:** this story writes only a new directory and slots pre-declared for it: a new crate needs no root `Cargo.toml` edit because members is the glob `crates/*`, and `src/lib.rs` already carries `#[cfg(feature = "llm")] mod llm;` — level 1 (stated in `story:workspace-skeleton` step 1 and `story:testkit-conformance-target` step 1; neither file exists to point at), unproven. Residual: three stories each rewrite one adjacent line in the testkit `[features]` and `[dependencies]` tables, and git's three-way merge conflicts on adjacent-line edits regardless of line ownership, so "may merge in any order" holds only if the pre-declared lines are separated by other content or the merges land sequentially
