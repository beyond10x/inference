---
format: aep.planning-md/3
id: story:endpoint-controller
kind: story
status: draft
title: 'Endpoint controller: VAD and turn-policy nodes complete turns'
summary: A deterministic energy VAD node and a turn-policy node open and complete inference.stream Turns as the single turn authority; no new noun.
relations:
- decomposes: epic:tools-and-turns
- serves: vision:inference
- depends_on: story:inbound-text-spans-and-envelope
- depends_on: story:audio-and-stt-contract
- depends_on: story:node-instance-lifecycle
- depends_on: story:text-pipeline-end-to-end
- depends_on: story:tool-authority-and-executor-nodes
scope:
- confidence: cited
  path: crates/inference-pipeline/Cargo.toml
- confidence: cited
  path: crates/inference-pipeline/src/nodes/mod.rs
- confidence: cited
  path: crates/inference-pipeline/src/nodes/turn_policy.rs
- confidence: cited
  path: crates/inference-pipeline/src/nodes/vad.rs
- confidence: inferred
  path: crates/inference-runtime/Cargo.toml
- confidence: cited
  path: crates/inference-runtime/tests/endpoint_controller.rs
- confidence: inferred
  path: crates/inference-testkit/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/src/stt.rs
- confidence: cited
  path: crates/inference-testkit/src/stt/synthetic_audio.rs
revision: 16
---
## Context

Turn completion is a control boundary declaring a whole input turn complete, distinct from the
commit of individual spans (`docs/design/inference.md:606`); committing an input turn is
serialized by a clear authority (699); endpointing, VAD and turn control are policy nodes (466;
`inference.graph.NodeRole` `policy`); VAD is not full turn-taking by itself (917); the reference
stack has a local VAD in the audio frontend (933) and topology B starts with VAD/endpointing
(762). `inference.stream.Turn` (`ess/domains/stream.yaml`) already has Open and Completed with
`OpenTurn` and `CompleteTurn`; this story adds no noun. local-voice-cascade endpoints from the STT
adapter or a simple energy VAD inside its own path; this story makes the dedicated nodes.

It runs after four siblings, and the `depends_on` edges record it:
`story:inbound-text-spans-and-envelope` (the in-memory Turn store and the event log that answer and
record the turn commands the nodes issue), `story:audio-and-stt-contract` (`inference-audio`
frames, the testkit `stt` feature and the mock recogniser), `story:node-instance-lifecycle` (the
node-instance trait both nodes implement) and `story:text-pipeline-end-to-end` (the mock source
and collector sink and the runtime loop that drives a run to Completed).

## Planned scope

- The VAD node (role `policy`): audio frames in (`inference-audio`), speech activity out on a
  Control-modality port: a deterministic energy threshold with attack and hold times as
  configuration; no model, no weights (Silero and sherpa-onnx VADs are adapter candidates,
  917-919, out of scope).
- The turn-policy node (role `policy`): activity in, turn control out: `OpenTurn` at speech onset
  when no turn is Open, `CompleteTurn` once the endpoint silence is reached; the single authority
  for turn commands in a pipeline (699); a second completion of the same turn is the
  `wrong-state` refusal. It may also take spans as input so that a turn does not complete while a
  span it saw is still Hypothesis, if the implementation needs that; completing a turn never
  commits a span (606).
- Downstream consumers read `TurnCompleted` as the end-of-input boundary; an ordinary text LLM
  needs a sufficiently finalized prompt per generation (778). Wiring that into the cascade is
  `story:cascade-read-only-tool`.
- Test: a synthetic audio fixture in the testkit (tone, silence, tone, silence; generated, no
  recording) emitted by the mock source of `story:text-pipeline-end-to-end`, through VAD and turn
  policy into its collector sink, the turn commands answered by the in-memory Turn store of
  `story:inbound-text-spans-and-envelope` and read back from its event log and the
  `inference.stream.Turns` view.

## Where it lands

- `crates/inference-pipeline/src/nodes/vad.rs` and
  `crates/inference-pipeline/src/nodes/turn_policy.rs`, new files in the crate
  `story:tool-authority-and-executor-nodes` puts its authority and executor nodes in
  (`inference-pipeline`: nodes, with `core` and declared port types as its dependencies, line 241;
  `inference-audio` is such a port type). Both implement the node-instance trait of
  `story:node-instance-lifecycle`. If that trait's crate forces the authority and executor nodes
  elsewhere, these two move with them, never alone.
- `crates/inference-testkit/src/stt/synthetic_audio.rs`, a new file behind the testkit's `stt`
  feature, the feature that already brings the audio dependency in
  (`story:audio-and-stt-contract`); no new feature.
- `crates/inference-runtime/tests/endpoint_controller.rs`, beside the A-02 test of
  `story:text-pipeline-end-to-end`.
- Nothing of `story:tool-call-lifecycle` (`ess/domains/tools.yaml`, `inference-tools`, the
  conformance target) is touched, so the two may share a wave once the edges above are met.

This story depends on `story:tool-authority-and-executor-nodes` for the module-declaration files both would otherwise create: that story creates `crates/inference-pipeline/src/nodes/mod.rs` with its two `mod` lines and adds `inference-tools` to `crates/inference-pipeline/Cargo.toml`; this story then adds `mod vad;` and `mod turn_policy;` to that `mod.rs` and `inference-audio` to that `Cargo.toml`. The synthetic audio fixture lives at `crates/inference-testkit/src/stt/synthetic_audio.rs`, a submodule of the `stt` module `story:testkit-conformance-target` pre-declares behind feature `stt`, so `crates/inference-testkit/src/lib.rs` is not touched.

## Domain relations

- Turn references Run, one: `ess/domains/stream.yaml`, `inference.stream.Turn`, relation `run`;
  the node opens turns in the run it executes in.
- Span references Turn, one, not owned (D1 of `executable-system-specification:inference`):
  `ess/domains/stream.yaml`, `inference.stream.Span`, relation `turn`; a Hypothesis span in a
  completed turn stays Hypothesis until the commit gate commits it.

## Acceptance

On the synthetic fixture the two nodes open a turn at each speech onset and complete it after the
hold time, the testkit event log recording `TurnOpened`, `TurnCompleted`, `TurnOpened`,
`TurnCompleted` in that order and `inference.stream.Turns` listing two Completed turns, with a
repeated `CompleteTurn` on a completed turn refused `wrong-state`, the nodes honouring
`inference.stream.OpenTurn/outcome/opened`, `inference.stream.CompleteTurn/outcome/completed`,
`inference.stream.CompleteTurn/outcome/wrong-state`,
`inference.stream.Turn/state/Completed/refuses/inference.stream.CompleteTurn` and
`inference.stream.Turn/transition/complete/by/inference.stream.CompleteTurn/completed`, and the
check below holding.

## Checks

1. Completing a turn commits no span (606): the mock recogniser of `story:audio-and-stt-contract`
   (testkit feature `stt`), handed the run's RunId and the TurnId of the first turn and scripted
   to emit one hypothesis and no commit, leaves that span Hypothesis in `inference.stream.Spans`
   after the turn is Completed.

## Out of scope

A learned VAD or endpointer adapter; the endpoint latency measure as a target (1020; emit it as
a metric only); tools.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). The repository has no commits and no code (`git log`:
no commits; no `crates/`, no root `Cargo.toml`), so every `crates/` path below is one this story or an
upstream story creates, and a symbol search has nothing to search: the surfaces are the paths the
body names, checked against `ess/` and the neighbour bodies.

- **Primary surface:** `crates/inference-pipeline/src/nodes/vad.rs` and `crates/inference-pipeline/src/nodes/turn_policy.rs`, two new files, one per node, in the crate `inference-pipeline` (nodes; `core` and declared port types as dependencies, `docs/design/inference.md:241`) that `story:pipeline-declaration` creates and whose `nodes/` module `story:tool-authority-and-executor-nodes` opens; this story creates no crate and no directory — cited (Where it lands, bullet 1 and closing paragraph)
- **Files:** `crates/inference-pipeline/src/nodes/mod.rs`, the lines `mod vad;` and `mod turn_policy;` added to the file the authority story creates with its own two — cited (closing paragraph; `depends_on story:tool-authority-and-executor-nodes` is in the store)
- **Files:** `crates/inference-pipeline/Cargo.toml`, one `inference-audio` line in `[dependencies]`, beside the authority story's `inference-tools` line and `story:capability-planning`'s `proptest` dev-dependency, both upstream — cited (closing paragraph)
- **Files:** `crates/inference-testkit/src/stt/synthetic_audio.rs`, new, behind feature `stt`, a submodule of the `stt` module; bullet 2 of Where it lands still spells it `crates/inference-testkit/src/synthetic_audio.rs`, and the closing paragraph (the round-2 fix, `review-result:tools-parallel-round-2`, outcome fixed 2026-10-18T22:14:25Z, which quotes the older path) supersedes it — cited
- **Files:** `crates/inference-runtime/tests/endpoint_controller.rs`, new, beside the A-02 test of `story:text-pipeline-end-to-end` (`crates/inference-runtime/tests/`, that story's Context) — cited (Where it lands, bullet 3)
- **Symbols:** `inference.stream.Turn` (`ess/domains/stream.yaml:46`; Open, Completed, transition `complete` `:61-68`), `OpenTurn` (`:236`), `CompleteTurn` with outcomes `completed` and `wrong-state` (`:255-270`), `TurnStateConflict` (`:204`), events `TurnOpened` (`:583`) and `TurnCompleted` (`:590`), views `Turns` (`:678`) and `Spans` (`:689`), `inference.graph.NodeRole` variant `policy` (`ess/domains/graph.yaml:49-51`) — cited; every one consumed, none declared or changed, and their Rust homes (`inference-text` for Turn and Span, `story:inbound-text-spans-and-envelope` step 2) are read, not edited
- **Also likely:** `crates/inference-testkit/src/stt.rs`, the parent module file that must gain `mod synthetic_audio;` for the fixture to be its submodule; the body says only that `src/lib.rs` is not touched, and the file name is the inference `story:testkit-conformance-target` and `story:audio-and-stt-contract` already recorded (`src/stt.rs`, not `stt/mod.rs`) — inferred
- **Also likely:** `crates/inference-testkit/Cargo.toml`, one optional `inference-audio` dependency under feature `stt`: the fixture emits `inference-audio` frames and a Cargo crate can name only its direct dependencies, so the `stt` feature "already brings the audio dependency in" (bullet 2) only if `inference-stt` re-exports `inference_audio`, which no body says — inferred
- **Also likely:** `crates/inference-runtime/Cargo.toml`, `[dev-dependencies]`: feature `stt` on `inference-testkit` (and `inference-audio` if the test names a frame type); no body names the runtime's dev-dependency on the testkit, and `review-result:tools-parallel-round-2` leaves this exact question open for this story and the authority story — inferred
- **Not touched:** `crates/inference-testkit/src/lib.rs` (body: "is not touched"); `ess/domains/*.yaml` and `ess/SKIPPED.md` ("this story adds no noun"; the five scenario ids the Acceptance names are among the 14 that `story:inbound-text-spans-and-envelope` moves to `passed`, its `## Scenarios`); `ess/domains/tools.yaml`, `crates/inference-tools`, `crates/inference-testkit/tests/conform.rs` and `tests/target/*` (bullet 4; confirmed by `review-result:tools-parallel-round-2`); root `Cargo.toml` (members glob `crates/*`), `Taskfile.yml`, `AGENTS.md`, `README.md`, `docs/design/inference.md` (named by no step) — cited
- **Documents:** none — cited; the Acceptance is one Rust integration test plus one check, both in `crates/inference-runtime/tests/endpoint_controller.rs`
- **Confidence:** high — the body names every file it creates or edits and the round-2 fix settled the shared registration files; the three inferred lines are one-line registration edits the named files force, in manifests that do not exist yet
- **Would collide with:** any unit editing `crates/inference-pipeline/src/nodes/mod.rs` or the `[dependencies]` table of `crates/inference-pipeline/Cargo.toml` (the authority story, ordered by `depends_on`; `story:tool-feedback-loop` and `story:tool-safe-retries` edit `tool_authority.rs` and `tool_executor.rs`, different files); any unit editing the testkit's `stt` module file or the `[dependencies]`/`[features]` tables of `crates/inference-testkit/Cargo.toml` (six stories record that file, all upstream of this one through `story:text-pipeline-end-to-end depends_on epic:contracts-and-testkit`); any unit adding to `[dev-dependencies]` of `crates/inference-runtime/Cargo.toml`; `Cargo.lock` once it exists (every dependency-adding unit; regenerated, not merged, so not recorded as an entry); and, without a shared file, any unit changing the node-instance trait signature in `inference-pipeline` or `Turn`/`CompleteTurn` in `inference-text`, an API break rather than a textual conflict — cited
- **Safety fact:** the nodes issue only `OpenTurn` and `CompleteTurn`, whose store, outcomes and `wrong-state` refusal already exist (`ess/domains/stream.yaml:236-270`) and are already claimed passing by `story:inbound-text-spans-and-envelope`, so this story moves no `ess/` line and no `ess/SKIPPED.md` line and cannot turn `tests/conform.rs` red, and every edit to an existing file is an additive registration line (`mod.rs`, `Cargo.toml`) — step 2 (`file:line`), unproven. Where the symbol search stops: whether `inference-audio` reaches the testkit through the `stt` feature (Cargo exposes direct dependencies only; the body's claim is step 1), and whether `inference-audio` is a "declared port type" `inference-pipeline` may depend on (`docs/design/inference.md:241`, the body's reading; `AGENTS.md` Rules keep that crate free of model, backend and provider implementations, and an energy threshold is none of those) — step 1, unproven
