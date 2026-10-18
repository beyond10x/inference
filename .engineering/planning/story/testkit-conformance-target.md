---
format: aep.planning-md/3
id: story:testkit-conformance-target
kind: story
status: draft
title: Testkit conformance target answers the inference.model scenarios
summary: inference-testkit holds in-memory stores and an in-process ConformanceTarget driven by ess-conformance from a test; the three inference.model scenarios pass and every unanswered scenario is named in ess/SKIPPED.md.
relations:
- decomposes: epic:contracts-and-testkit
- depends_on: story:workspace-skeleton
- serves: vision:inference
scope:
- confidence: cited
  path: AGENTS.md
- confidence: cited
  path: Taskfile.yml
- confidence: cited
  path: crates/inference-testkit
- confidence: cited
  path: crates/inference-testkit/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/src/graph.rs
- confidence: cited
  path: crates/inference-testkit/src/lib.rs
- confidence: inferred
  path: crates/inference-testkit/src/llm.rs
- confidence: inferred
  path: crates/inference-testkit/src/model.rs
- confidence: inferred
  path: crates/inference-testkit/src/policy.rs
- confidence: inferred
  path: crates/inference-testkit/src/session.rs
- confidence: inferred
  path: crates/inference-testkit/src/stream.rs
- confidence: inferred
  path: crates/inference-testkit/src/stt.rs
- confidence: inferred
  path: crates/inference-testkit/src/tools.rs
- confidence: inferred
  path: crates/inference-testkit/src/tts.rs
- confidence: cited
  path: crates/inference-testkit/tests/conform.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/graph.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/mod.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/model.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/policy.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/session.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/stream.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/tools.rs
- confidence: cited
  path: ess/SKIPPED.md
revision: 26
---
## Context

The epic names `inference-testkit` as the target that answers `ExecuteCommand` and `QueryView` for `inference.model` and `inference.stream`, and its exit reads: executed by the testkit's own Rust runner, reported through `ess verify conform report`. In ess 0.60.0 `ess verify conform run` accepts only its built-in reference targets, `billing`, `oracle-fixture` and `interpreted` (`ess verify conform run --help`), so a Rust target cannot be run by the CLI, and the route has two parts. The suite is executed in process by the `ess-conformance` crate's runner (git tag `0.60.0` of `github.com/beyond10x/ess`, `crates/verify/ess-conformance/src/runner.rs`: `Runner::run<T: ConformanceTarget>` returns an `ExecutedRun` whose report holds one terminal status per scenario) against an implementation of `ess_conformance::target::ConformanceTarget` in the testkit's test tree; that is the Rust runner of the exit. The verdict is then not the run's own document but the one `ess verify conform report` writes: the test serializes the per-scenario statuses as an `ess-conformance-results/1` document (`format`, `completed_at`, `results` of `{scenario_id, status}`, `status` one of `passed`, `failed`, `error` and `unsupported`; `ess verify conform report --help`) and runs `ess verify conform report --suite <dir>/suite.json --results <dir>/results.json --report-out <dir>/report.json`, which admits the suite, refuses a scenario with no result or a result for a scenario outside the suite, and writes the `ess-conformance-report/2` whose `producer_profile` is `external-scenario-status/1`. That report and the suite beside it are what `aep plan artifact evidence --from <dir>/report.json --suite <dir>/suite.json <story>` records, so the test leaves the directory in place, red or green, and prints its path. The exit of the epic is met by that report.

`inference-testkit` is `docs/design/inference.md:202` and line 244 (mocks, protocol validation, replay fixtures; contracts and test-only utilities). This story gives it the in-memory stores, the target, the runner route and the feature scheme the mocks sit behind; the mocks come with the three domain contract stories. The repository ships no binary (`AGENTS.md`), so the target lives in the test tree of the testkit, with `ess-conformance` and `ess-primitives` as dev-dependencies pinned to the release `ess/ess-inputs.yaml` requires (`requires: ess 0.60`: git tag `0.60.0`; the workspace rule is the newest ESS release). The library part of the crate exposes the stores and pulls no ess crate, which is what keeps the A-01 dependency tree of the STT story free of test tooling.

Later stories of three epics land on this crate: the two stream stories, `story:pipeline-declaration` (`inference.graph`, `inference.policy`), `story:tool-call-lifecycle` (`inference.tools`) and `epic:session-facade`. So that two of them can be worked at once without meeting in one file, the target and the stores are split per domain from the start and `ess/SKIPPED.md` has one section per domain: a later story fills its domain's modules and edits its domain's section, and touches nothing else this story writes.

## Domain relations

- `inference.model.Adapter` references `inference.model.Model`, cardinality one. Inferable: `ess/domains/model.yaml`, entity `inference.model.Adapter`, relation `model`. The scenario declares an adapter with a literal `model_id` and no prior `DeclareModel` (`.engineering/drafts/suite.json`, `inference.model.DeclareAdapter/outcome/declared`), so the store does not require the model to exist: a reference, not a foreign-key check.

## Steps

1. `crates/inference-testkit/src/`: `lib.rs` declares one store module per domain of `ess/system.yaml` (`model`, `policy`, `graph`, `stream`, `tools`, `session`) and three feature-gated mock modules `stt`, `llm` and `tts` (each behind `#[cfg(feature = "...")]`), every module an empty file until its story fills it, so that no later story adds a line to `lib.rs`. The `model` module is filled here: an in-memory store per entity of `inference.model`, the views `inference.model.Models` and `inference.model.Adapters` with the fields `model.yaml` lists and `read_your_writes` consistency, fed by the constructors of `inference-core`, every emitted event recorded in order. Identifiers come from a counter rendered as a Uuid, so two runs of one suite report the same thing. `Cargo.toml` declares `[features]` `stt = []`, `llm = []` and `tts = []`; each contract story turns its own into `["dep:<its crate>"]` and adds that optional dependency, and the conformance target needs none of the three.
2. `crates/inference-testkit/tests/conform.rs` and `tests/target/`: `tests/target/mod.rs` implements `ConformanceTarget`, translating each `SemanticCommandRequest` into a store call and each `SemanticViewRequest` into view rows by the domain prefix of the name, through one module per domain (`tests/target/model.rs`, `stream.rs`, `graph.rs`, `policy.rs`, `tools.rs`, `session.rs`), one fresh store per scenario; `model.rs` answers the two commands and two views here, and every other module answers `unsupported` for every name until its story fills it. A command the target does not answer yet is reported `unsupported`, never faked, and no outcome is decided from bookkeeping the target did itself. `conform.rs` is the driver: it synthesizes the suite into a fresh directory under `CARGO_TARGET_TMPDIR` with `ess verify conform synthesize --path ess --out <dir>/suite.json` run from the repository root (`ess` on PATH, as `task check` already assumes), runs it with `ess_conformance::Runner` against the target, writes `<dir>/results.json` as `ess-conformance-results/1` from the run's per-scenario statuses, runs `ess verify conform report --suite <dir>/suite.json --results <dir>/results.json --report-out <dir>/report.json --implementation inference-testkit` and fails unless its exit is 0, reads `<dir>/report.json`, prints `<dir>`, and asserts on the report: `failed` is 0, `error` is 0, the set of `unsupported` ids equals the set of ids named in `ess/SKIPPED.md`, and `passed` contains the three `inference.model` ids.
3. `ess/SKIPPED.md`: one `## <domain>` section per domain of `ess/system.yaml` with an unanswered scenario, under it one line `- <scenario id>: <reason>` per scenario the target does not answer yet, the reason naming the story or epic that will: the 60 `inference.stream` ids name `story:outbound-text-generations-and-segments` or `story:inbound-text-spans-and-envelope`; the 85 `inference.graph` ids name `epic:graph-minimum`; the 3 `inference.tools` ids `epic:tools-and-turns`; the `inference.policy` id names the epic whose pipelines first reference a profile, `epic:graph-minimum` (`ess/domains/graph.yaml`, entity `inference.graph.Pipeline`, relation `profile`); the `inference.session` id `epic:session-facade`. Every later story removes exactly the ids it makes pass from its domain's section; the equality assertion turns a stale line into a red test.
4. `Taskfile.yml`: `check` runs `cargo clippy --workspace --all-targets --all-features -- -D warnings` and `cargo test --workspace --all-features` in place of the skeleton story's two commands, so the three feature-gated modules are built and tested as their stories fill them. `AGENTS.md` Commands gains `cargo test -p inference-testkit --test conform` as the way to run the target alone, names the three testkit features, and says where the test leaves `suite.json` and `report.json`.

## Acceptance

`cargo test -p inference-testkit --test conform` passes, having written `<dir>/results.json` and run `ess verify conform report --suite <dir>/suite.json --results <dir>/results.json --report-out <dir>/report.json` with exit 0, with the `ess-conformance-report/2` at `<dir>/report.json` showing `passed` containing `inference.model.DeclareModel/outcome/declared`, `inference.model.DeclareAdapter/outcome/declared` and `inference.model.DeclareAdapter/outcome/refused`, `failed` and `error` at 0, and `unsupported` equal to the ids named in `ess/SKIPPED.md`.

## Out of scope

Any `inference.stream` command or view (the two stream stories); mocks (the domain contract stories); the optional dependencies behind `stt`, `llm` and `tts` (each contract story adds its own); a committed suite file, since the suite is synthesized at test time and follows `ess/`.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-testkit` — cited (Steps 1-2; `docs/design/inference.md:202`, `:244`); the crate does not exist yet, `story:workspace-skeleton` creates `crates/` and this story creates the testkit
- **Files:** `crates/inference-testkit/Cargo.toml` (`[features] stt = []`, `llm = []`, `tts = []`; dev-dependencies `ess-conformance`, `ess-primitives` at git tag `0.60.0`), `crates/inference-testkit/src/lib.rs`, `crates/inference-testkit/tests/conform.rs`, `crates/inference-testkit/tests/target/mod.rs`, `crates/inference-testkit/tests/target/{model,stream,graph,policy,tools,session}.rs`, `Taskfile.yml` (`check` gains `--all-targets --all-features` clippy and `--all-features` test), `AGENTS.md` (Commands block, lines 24-31) — cited
- **Symbols:** `ess_conformance::target::ConformanceTarget` (`crates/verify/ess-conformance/src/target.rs:102`, `execute_command` :183, `query_view` :225), `Runner::run` (`runner.rs:376`), `ExecutedRun` (`runner.rs:281`), `SemanticCommandRequest`, `SemanticViewRequest`; views `inference.model.Models` (`ess/domains/model.yaml:245`) and `inference.model.Adapters` (`:260`), both `read_your_writes` — cited
- **Also likely:** `crates/inference-testkit/src/{model,policy,graph,stream,tools,session,stt,llm,tts}.rs` — inferred; the story names the nine modules and says each is "an empty file", the `src/<name>.rs` form is the edition default and `src/<name>/mod.rs` would satisfy the body equally
- **Documents:** `ess/SKIPPED.md` (new; one `## <domain>` section per domain of `ess/system.yaml`, 150 ids today), `AGENTS.md` Commands — cited
- **Not touched:** root `Cargo.toml` (members glob `crates/*` from `story:workspace-skeleton`; features and dev-deps are crate-level), `crates/inference-core` (read for its constructors), `ess/domains/*.yaml`, `ess/system.yaml`, `README.md` — inferred from the Steps naming no path there
- **Confidence:** high — the story names every file by path; the only two that pre-exist in the tree (`Taskfile.yml`, `AGENTS.md`) are 12 and 53 lines and the edited blocks are identified
- **Would collide with:** any unit touching `crates/inference-testkit` (the later stream, graph, tools and session stories fill files this story creates, so they sequence after it); any unit editing `Taskfile.yml` `check` or the `AGENTS.md` Commands block (`story:workspace-skeleton` writes the same lines this story replaces, ordered by `depends_on`); and, without sharing a file, any unit that changes the scenario set `ess verify conform synthesize --path ess` emits (an edit to `ess/domains/*.yaml` or a new domain in `ess/system.yaml`), because the `unsupported == ess/SKIPPED.md` equality in `tests/conform.rs` turns one added or removed scenario into a red test unless that unit edits `ess/SKIPPED.md` too — cited (Step 2 assertion, Step 3)
- **Safety fact:** `ess/SKIPPED.md` sits inside `ess/` and changes neither `ess specify validate --path ess` nor the synthesized suite, because `ess/ess-inputs.yaml:3-11` lists the seven specification documents explicitly and a Markdown file beside them is not one of them — step 2 (pointed at the file), unproven: not run with the file present
