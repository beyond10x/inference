---
format: aep.planning-md/3
id: story:workspace-skeleton
kind: story
status: draft
title: Cargo workspace skeleton; inference-core carries the model vocabulary
summary: A Cargo workspace under crates/ with task check running fmt, clippy and test; inference-core declares the inference.model vocabulary and the shared identifiers.
relations:
- decomposes: epic:contracts-and-testkit
- serves: vision:inference
scope:
- confidence: cited
  path: AGENTS.md
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: README.md
- confidence: cited
  path: Taskfile.yml
- confidence: cited
  path: crates/inference-core
- confidence: inferred
  path: crates/inference-core/Cargo.toml
- confidence: inferred
  path: crates/inference-core/src/lib.rs
revision: 11
---
## Context

The repository holds `docs/design/inference.md`, `ess/` and the planning store, and no code. Every later story of `epic:contracts-and-testkit` adds a crate or a module to one Cargo workspace, so the workspace, its lints and its check task come first: this is the prefactoring the decomposition rests on, and the other seven stories depend on it, directly or through one another. The crate list is `docs/design/inference.md:180-227`; this story creates `crates/inference-core/` only (line 191: events, IDs, errors, features, lifecycle; line 233: minimal shared primitives, small stable utility dependencies only). `inference-core` carries no concrete model, backend or provider implementation (`AGENTS.md`, Rules), and the repository ships no binary.

The vocabulary `inference-core` declares is `ess/domains/model.yaml`, read as written: the newtypes `inference.model.ModelId` and `inference.model.AdapterId`; the enums `inference.model.Capability` (the nine rows of `docs/design/inference.md:419-427`), `inference.model.Modality` (lines 288-299), `inference.model.Locality` (line 438) and `inference.model.ExecutionForm` (lines 876-880); the entities `inference.model.Model` and `inference.model.Adapter` with their fields; the error `inference.model.NonPositiveConcurrency`; the commands `inference.model.DeclareModel` and `inference.model.DeclareAdapter` as constructors returning the entity and its event, or the declared error; the events `inference.model.ModelDeclared` and `inference.model.AdapterDeclared`. Serialized names are the ESS names (`output_streaming`, `same_process`, `in_process`), because that is what the synthesized scenarios send (`.engineering/drafts/suite.json`, `inference.model.DeclareAdapter/outcome/refused`).

The shared identifiers live here too, because `inference-text` may depend on `inference-core` only (line 234) and its spans carry a run id and a stream id: `inference.graph.RunId`, `inference.graph.NodeId`, `inference.stream.StreamId`, `inference.stream.GenerationId` and `inference.session.SessionId`, each the newtype of Uuid that `ess/` declares. They are values here; the entities they identify arrive with their own epics (`epic:graph-minimum`, `epic:session-facade`), and this story defines no Run, Node or Session.

## Domain relations

- `inference.model.Adapter` references `inference.model.Model`, cardinality one from the adapter side; a model may have zero or many adapters and stands on its own, so dropping an adapter changes nothing about its model and a model may exist before any adapter does. Inferable: `ess/domains/model.yaml`, entity `inference.model.Adapter`, relation `model` (via `model_id`).

## Steps

1. Root `Cargo.toml`: a workspace with `members = ["crates/*"]`, workspace package metadata (edition, `rust-version`, `license = "Apache-2.0"`, which is what `LICENSE` is), workspace lints with `unsafe_code = "forbid"`. The glob takes every later crate under `crates/` without an edit to the members list; the one P0 member outside `crates/`, `examples/stt-only/`, is added as `"examples/*"` by `story:audio-and-stt-contract` when it creates it, and no other story of this epic or of `epic:graph-minimum` edits the list. No comments in any manifest (`AGENTS.md`). `target/` is already ignored.
2. `crates/inference-core/`: the types above with `serde` and `uuid`; the three unit tests of `## Checks`.
3. `Taskfile.yml`: `check` runs, after the two existing commands, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`. `AGENTS.md` Commands lists them; its sentence that they join as code lands goes.
4. `README.md`: how to build and check, in two lines.

## Acceptance

`task check` exits 0 running `ess specify validate`, `aep plan artifact validate`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` on a workspace whose one crate `inference-core` passes the three unit tests of `## Checks`, which together observe the `inference.model` vocabulary.

## Checks

1. `DeclareModel` with the seven inputs `ess/domains/model.yaml` lists returns the `Model` entity carrying them and a `ModelDeclared` event carrying the generated `model_id`, `family` and `checkpoint`, the payload the command declares.
2. `DeclareAdapter` with `max_concurrency` 0, and again with a negative value, is refused as `NonPositiveConcurrency` with `submitted` equal to the value sent and nothing declared; with `max_concurrency` 1 it returns the `Adapter` entity and an `AdapterDeclared` event carrying `adapter_id`, `model_id` and `capabilities`.
3. Every variant of `Capability` (the nine), `Modality`, `Locality` and `ExecutionForm` serializes to the name `ess/domains/model.yaml` lists (`output_streaming`, `Control`, `same_process`, `in_process`) and deserializes back, and each shared identifier newtype round-trips as the Uuid string it wraps.

## Out of scope

The conformance target and the testkit (next story); every other crate of the design list; the event envelope (declared in `ess/` by the inbound-text story before code names it); any example.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-core/`, the one crate this story creates — cited (body Context and step 2; `docs/design/inference.md:191,233`; `crates/` absent in the tree)
- **Files:** `Cargo.toml` at the root, new: workspace `members = ["crates/*"]`, package metadata, lints — cited (step 1; absent in the tree)
- **Files:** `Taskfile.yml`, the `check` task gains the three cargo commands after the two at `Taskfile.yml:11-12` — cited (step 3)
- **Files:** `AGENTS.md`, the Commands block at `AGENTS.md:26-31` lists them and the clause "format, lint and tests join as code lands" at `AGENTS.md:27` goes — cited (step 3)
- **Files:** `README.md`, two lines on build and check — cited (step 4)
- **Symbols:** `ModelId`, `AdapterId`, `Capability`, `Modality`, `Locality`, `ExecutionForm`, `Model`, `Adapter`, `NonPositiveConcurrency`, `DeclareModel`, `DeclareAdapter`, `ModelDeclared`, `AdapterDeclared` (`ess/domains/model.yaml:18-242`); `RunId`, `NodeId` (`ess/domains/graph.yaml:39,23`), `StreamId`, `GenerationId` (`ess/domains/stream.yaml:30,34`), `SessionId` (`ess/domains/session.yaml:17`), each a newtype of Uuid — cited
- **Also likely:** `crates/inference-core/Cargo.toml` and `crates/inference-core/src/lib.rs` — inferred: a Cargo library crate needs both, and `lib.rs` is the module list the later `inference-core` stories add a module to; the body names only the directory
- **Documents:** `AGENTS.md` and `README.md` as above; `ess/` is read, not changed (`ess/domains/model.yaml` "read as written", every noun already declared) — cited
- **Confidence:** high — every path is named in the body and its presence or absence confirmed in the tree (`Cargo.toml`, `crates/`, `examples/` absent; `Taskfile.yml`, `AGENTS.md`, `README.md` present)
- **Would collide with:** any unit touching the root `Cargo.toml` (members, metadata, lints), the `check` task of `Taskfile.yml`, the Commands block of `AGENTS.md`, `README.md`, or `crates/inference-core/` (its `src/lib.rs` module list and its `Cargo.toml` dependency list in particular)
- **Safety fact:** the tree holds no Rust and no commits (`git log`: "does not have any commits yet"; `Cargo.toml`, `Cargo.lock`, `crates/`, `examples/` absent; `.gitignore:1` already ignores `/target`), so the new workspace and crate can break no existing build, and the edits to existing files are additive except the one clause at `AGENTS.md:27` — step 2 (`ls`, `git log`), unproven. Where the symbol search stops: the wire names `output_streaming`, `same_process`, `in_process` (`ess/domains/model.yaml:31,52,58`) are what `.engineering/drafts/suite.json` sends (30, 5 and 2 occurrences) and what the next story replays; `Modality` is PascalCase (`Control`, line 46) while the other three enums are snake_case, so one `rename_all` cannot cover all four — step 2, unproven
