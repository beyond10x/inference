---
format: aep.planning-md/3
id: story:audio-and-stt-contract
kind: story
status: implemented
title: STT contract with a mock recogniser, usable alone
summary: inference-stt is the recogniser contract with its capabilities declared apart; the testkit mock recogniser behind feature stt passes §23.1 test 1; an stt-only example depends on inference-stt and the mock only (A-01).
relations:
- decomposes: epic:contracts-and-testkit
- depends_on: story:inbound-text-spans-and-envelope
- serves: vision:inference
- depends_on: story:audio-vocabulary
scope:
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: crates/inference-stt
- confidence: cited
  path: crates/inference-testkit/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/src/stt.rs
- confidence: inferred
  path: crates/inference-testkit/tests/stt.rs
- confidence: cited
  path: examples/stt-only
revision: 14
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T06:21:13Z", actor: "human:timo", revision: 12, decided_on: {"recorded":{"review_outcome":3}}}
- {from: "proposed", to: "active", at: "2026-10-19T06:21:13Z", actor: "human:timo", revision: 13, decided_on: {"recorded":{"review_outcome":3}}}
- {from: "active", to: "implemented", at: "2026-10-19T06:43:32Z", actor: "human:timo", revision: 14, decided_on: {"recorded":{"test_result":1,"review_outcome":11,"verification":1}}}
---
## Context

`docs/design/inference.md:350-362`, the STT contract: input is timestamped audio or an audio stream, output is hypotheses, revisions and committed spans with optional metadata (lines 352-353); batch transcription, incremental audio ingestion and output streaming are separately declared capabilities (line 358); provisional and committed spans are distinct (line 359); a chunked offline recogniser wrapped in a streaming node must not claim native incremental recognition when it processes complete chunks in independent calls (line 362). The crate is `inference-stt` (line 194; line 236: depends on `core`, `audio`, `text`). The frames it consumes are `inference-audio` (line 193; line 235), created by `story:audio-vocabulary`, where the position of a frame is a sample clock position in audio frame time; the spans it produces and the envelope that sequences them are `story:inbound-text-spans-and-envelope`. The contract projects the declared nouns (answer to Q2 in `executable-system-specification:inference`): an implementation is an `inference.model.Adapter` that declares its capabilities, consumes audio frames and produces the `inference.stream` span commands. Incremental audio ingestion is `incremental_input` and output streaming is `output_streaming` (`inference.model.Capability`, `docs/design/inference.md:419-427`); batch transcription is what remains when neither is claimed (lines 45-49).

A-01 (line 1137) is this story: an example depends on `inference-stt` and one mock adapter only, and `cargo tree` shows neither `inference-runtime` nor `inference-tts`; a consumer interested only in STT pulls in no TTS, no LLM server, no runtime and no transport (line 41). The mocks of the testkit sit behind the cargo features `stt`, `llm` and `tts` that `story:testkit-conformance-target` declares, each empty until its story fills it and each enabling only the optional dependency on its contract crate; the conformance target needs none of them, and `task check` already runs clippy and test with `--all-features`. This story fills `stt` only. The example is `examples/stt-only/` (line 215), a workspace member with `publish = false`, and the one P0 member outside `crates/`, so this story adds `"examples/*"` to the root `members` list (`story:workspace-skeleton`, step 1).

## Domain relations

- The mock recogniser is an `inference.model.Adapter` referencing one `inference.model.Model`: inferable, `ess/domains/model.yaml`, entity `inference.model.Adapter`, relation `model`; every adapter passes the tests for the capabilities it claims (`docs/design/inference.md:899`).
- The spans the mock emits are `inference.stream.Span`, each referencing a turn and a run: inferable, `ess/domains/stream.yaml`, entity `inference.stream.Span`, relations `turn` and `run`. The mock is handed the `RunId` and `TurnId` it emits under; no Run or Turn store is consulted.
- Audio frames have no entity relation in the design: a frame is a value that flows on an edge, and edges are `epic:graph-minimum`. No relation is declared for them here (their declaration is `story:audio-vocabulary`).

## Steps

1. `crates/inference-stt/`: the recogniser contract (the design calls it the STT contract; the trait name is open to the implementor): declare the adapter record; accept `inference-audio` frames and an end-of-input marker; emit `TranscriptHypothesis`, `TranscriptRevision` and `TranscriptCommitted` on a `StreamId` with a per-stream sequence, enveloped as `story:inbound-text-spans-and-envelope` defines; answer `CommittedSpanImmutable` to a revision of a committed span. Whether input may arrive before the end of the utterance and whether output may precede the end of input are two declared capabilities read from the adapter record, never implied by the shape of the API.
2. `inference-testkit`, feature `stt`: `crates/inference-testkit/Cargo.toml` gains the optional dependency on `inference-stt` and `stt = ["dep:inference-stt"]` in place of the `stt = []` the testkit story declared; the `stt` module, declared empty behind `#[cfg(feature = "stt")]` by that story, gains a scripted mock recogniser replaying a fixed script (frame count to hypothesis, revision, commit) in two configurations: one claiming `incremental_input` and `output_streaming`, one claiming neither and answering only after end of input (line 362). §23.1 test 1 (line 1105) as a Rust test against the mock: after the mock commits a span, a revision of that span is refused with `CommittedSpanImmutable` and the text of the span is unchanged; a second test shows the incremental configuration emitting a hypothesis before end of input and the batch configuration emitting nothing before it. `story:llm-contract` and `story:tts-contract` make the same two-line edit to `Cargo.toml` for their own crates and fill their own modules, and `story:pipeline-declaration` adds its `inference-pipeline` dependency line to the same table; no line of the testkit is shared between them, so they may merge in any order.
3. `examples/stt-only/`: depends on `inference-stt` and on `inference-testkit` with `features = ["stt"]` only; feeds a frame fixture to the mock and prints committed spans; the root `Cargo.toml` `members` gains `"examples/*"`. A test in that package runs `cargo tree -p stt-only -e normal` (without `--all-features`, so the workspace-wide flag of `task check` does not unify features into it) through `std::process::Command` and asserts the output names neither `inference-tts`, `inference-llm`, `inference-runtime` nor `ess-conformance`.

## Acceptance

`task check` passes with the two mock-recogniser tests and the `stt-only` tree test of `## Checks` green.

## Checks

1. §23.1 test 1: after the mock commits a span, a revision of that span is refused as `CommittedSpanImmutable` and the text of the span is unchanged.
2. The incremental configuration emits a hypothesis before end of input and the batch configuration emits nothing before it, each matching the capabilities its adapter record declares.
3. A-01: `cargo tree -p stt-only -e normal` names `inference-stt`, `inference-audio` and `inference-testkit` and neither `inference-tts`, `inference-llm`, `inference-runtime` nor `ess-conformance`.

## Out of scope

Audio frames, formats and sample clocks (`story:audio-vocabulary`). Languages, audio formats, sample rates and channel limits as adapter descriptors (line 357) and speech activity, timing and diarization metadata (lines 360-361): not declared in `ess/` and not required by the exit of the epic; `epic:local-stt-adapter` extends the specification when a real backend needs them. VAD and endpointing (`epic:tools-and-turns`). Wrapping the mock as a graph node with ports (`epic:graph-minimum`).

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). The repository has no commits and no code (`git ls-files`
is empty; no `crates/`, `examples/` or root `Cargo.toml`), so every path below is one this story or a
sibling creates.

- **Primary surface:** `crates/inference-stt` — cited: step 1; `docs/design/inference.md:194`, `:236` (new crate; depends on `core`, `audio`, `text`)
- **Files:** `crates/inference-stt` (the whole new crate) — cited: step 1
- **Files:** `crates/inference-testkit/Cargo.toml` — cited: step 2, two lines only: the optional dependency on `inference-stt` and `stt = ["dep:inference-stt"]` replacing the `stt = []` of `story:testkit-conformance-target`
- **Files:** `examples/stt-only` (new workspace member, `publish = false`, holding the `cargo tree` test of step 3) — cited: step 3; `docs/design/inference.md:215`
- **Files:** `Cargo.toml` (root: `[workspace] members` gains `"examples/*"`) — cited: step 3; `story:workspace-skeleton` step 1 names this story as the one adder
- **Symbols:** `TranscriptHypothesis`, `TranscriptRevision`, `TranscriptCommitted` (`ess/domains/stream.yaml:835-858`), `CommittedSpanImmutable` (`:243`; `ReviseSpan` outcome `immutable`, `:457-462`), `incremental_input`, `output_streaming` (`ess/domains/model.yaml:31-32`), the `inference.model.Adapter` record (`model.yaml:115-141`), `StreamId`, `RunId`, `TurnId` — cited; all consumed from `inference-text` and `inference-core`, none declared by this story
- **Also likely:** `crates/inference-testkit/src/stt.rs` — inferred: the `stt` module behind `#[cfg(feature = "stt")]` is named by step 2 and pre-created empty by `story:testkit-conformance-target` step 1, but neither body names the file (`stt.rs` or `stt/mod.rs`)
- **Also likely:** `crates/inference-testkit/tests/stt.rs` — inferred: the two mock-recogniser tests (§23.1 test 1; incremental vs batch) have no stated location; placed by analogy with `tests/envelope.rs` of `story:inbound-text-spans-and-envelope`; they may instead be `#[cfg(test)]` inside the module, in which case this file does not exist
- **Not touched:** `crates/inference-testkit/src/lib.rs` (module pre-declared: step 2, testkit step 1), `Taskfile.yml` and `AGENTS.md` (`task check` already runs `--all-features`: Context), `ess/` and `ess/SKIPPED.md` (no new noun; no scenario in Acceptance; the extra STT descriptors are deferred to `epic:local-stt-adapter`: Out of scope) — cited
- **Documents:** none — cited: the Acceptance is `task check` with three Rust tests, and the epic's Context names the envelope and the audio nouns as the only two `ess/` declarations of this epic, both other stories'
- **Confidence:** high — the body names every landing path itself and the design's crate tree (`docs/design/inference.md:190-227`) confirms each; only two file names inside `crates/inference-testkit` are inferred
- **Would collide with:** any unit editing `crates/inference-testkit/Cargo.toml` (`[dependencies]` or `[features]`; the body admits three sibling edits to that file) and any unit editing the root `Cargo.toml` `[workspace]` table; nothing else, since `crates/inference-stt` and `examples/stt-only` are created here and the `stt` module is this story's alone — cited
- **Safety fact:** the two lines this story writes into `crates/inference-testkit/Cargo.toml` are separated from the sibling `llm`, `tts` and `inference-pipeline` lines by at least one unchanged line, so a textual three-way merge sees no adjacent hunks and "may merge in any order" (Context) holds — level 1, stated by the body; walked at level 3: `story:testkit-conformance-target` step 1 declares `stt = []`, `llm = []`, `tts = []` as consecutive lines, and git reports edits to adjacent lines as a conflict, so the claim fails unless the testkit story or the first-merged sibling leaves a line between each pair; the file does not exist yet (same question left open at `.engineering/planning/review-result/contracts-parallel-round-2.md:47`), unproven
