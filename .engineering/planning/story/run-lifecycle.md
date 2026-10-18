---
format: aep.planning-md/3
id: story:run-lifecycle
kind: story
status: draft
title: 'Run lifecycle: a run of a planned pipeline moves through its states by command and refuses the rest'
summary: inference-runtime crate, the eight Run commands, DescribeNodeInstance, 66 scenarios
relations:
- decomposes: epic:graph-minimum
- depends_on: story:capability-planning
- serves: vision:inference
- depends_on: story:bounded-edges
scope:
- confidence: cited
  path: crates/inference-runtime
- confidence: cited
  path: crates/inference-runtime/src/lib.rs
- confidence: inferred
  path: crates/inference-runtime/src/run.rs
- confidence: inferred
  path: crates/inference-testkit/src/graph.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/graph.rs
- confidence: cited
  path: ess/SKIPPED.md
revision: 9
---
# Story: run-lifecycle

## Outcome

A consumer creates a run of a planned pipeline and the runtime moves it Created, Validating, Preparing, Active, Draining, Completed by command, with Failed and Cancelled explicit and every out-of-order command refused with the current state, so the run lifecycle is observable before any event flows.

## Context

Fourth story of `epic:graph-minimum`; it adds the Run state machine to `crates/inference-runtime` (graph execution and operational policies; depends on `inference-pipeline`, `inference-core` and runtime utilities, `docs/design/inference.md:242`; the executor is an implementation detail that stays out of public contracts, line 665). The crate itself, its `Cargo.toml` and `src/lib.rs`, is created by `story:bounded-edges`, which runs beside validation and planning; the `depends_on` edge to that story records those two shared files as its reason, and this story adds a module of its own and one line to `src/lib.rs`. `inference.graph.Run` (`ess/domains/graph.yaml`) has the states of the design (`docs/design/inference.md:671`) plus Failed and Cancelled (line 673; decision D6 of `executable-system-specification:inference`) and eight commands: `CreateRun`, `ValidateRun`, `PrepareRun`, `ActivateRun`, `DrainRun`, `CompleteRun`, `FailRun` (issued by the runtime, D10, line 1006) and `CancelRun` (cancellation of delivery is distinct from cancellation of computation, line 641). Each moving command answers `wrong-state` with `inference.graph.RunStateConflict { current }` from any state its transition does not leave. `DescribeNodeInstance` creates an `inference.graph.NodeInstance` in its first state, Described; the further states come with `story:node-instance-lifecycle`.

Two surfaces again: the commands, answered by the testkit runner for the 66 scenarios listed below, and a Rust entry point in `inference-runtime` that takes the Planned form from `story:capability-planning` and drives the same state machine. The runner answers `CreateRun` for any `pipeline_id` (the scenarios create runs on ids that were never declared, for example `inference.graph.Run/transition/cancel/by/inference.graph.CancelRun/cancelled`); the Rust entry point cannot be given anything but a planned pipeline, which is how `docs/design/inference.md:519` and `651` are honoured.

## Steps

1. Specification: nothing to add for the Run; `ess specify validate --path ess` stays valid.
2. Contract: `Run`, `RunId`, the state enum, `RunStateConflict`, `NodeInstance`, `NodeInstanceId` in a module of `inference-runtime` beside the queue module of `story:bounded-edges`; the Planned form of the pipeline is the only constructor input.
3. Behaviour: the transition table of graph.yaml (`validate` Created to Validating; `prepare` Validating to Preparing; `activate` Preparing to Active; `drain` Active to Draining; `complete` Draining to Completed; `fail` from Validating, Preparing, Active; `cancel` from Created, Validating, Preparing, Active, Draining); a refused command moves nothing and emits nothing (the refusal scenarios assert the view is unchanged).
4. Testkit: the runner answers the nine commands and the views `inference.graph.Runs` (with `state`) and `inference.graph.NodeInstances`; `ess/SKIPPED.md` loses exactly the 66 ids listed under Scenarios (`story:testkit-conformance-target`, step 3).
5. Tests: the scenarios; a Rust test that a run created from a Planned form reaches Completed through the six states by command and that `CancelRun` from each non-terminal state reaches Cancelled.

## Domain relations

All `inferable`, from `ess/domains/graph.yaml`:

- `Run → Pipeline`, many-to-one, references; a pipeline is not owned by its runs and survives them: entity `inference.graph.Run`, relation `pipeline`. Lifecycle coupling: `CreateRun` has one outcome and no guard on the pipeline state, so the runner creates a run for any id; the design requires a plan before anything runs (`docs/design/inference.md:519`, `651`), which the Rust entry point enforces by type.
- `Run → NodeInstance`, one-to-many, run owns instance; an instance does not outlive its run: relation `instances`.
- `NodeInstance → Node`, references one: entity `inference.graph.NodeInstance`, relation `node`.

## Acceptance

The runtime passes the 66 scenarios listed under Scenarios (today `unsupported`, each named in `ess/SKIPPED.md`) with none of them left in that file, among them `inference.graph.CreateRun/outcome/created`, `inference.graph.Run/transition/complete/by/inference.graph.CompleteRun/completed`, `inference.graph.Run/transition/cancel/by/inference.graph.CancelRun/cancelled`, `inference.graph.Run/transition/fail/by/inference.graph.FailRun/failed`, `inference.graph.Run/state/Created/refuses/inference.graph.PrepareRun` and `inference.graph.DescribeNodeInstance/outcome/described`, and the Rust test drives a run built from a Planned form to Completed and to Cancelled from every non-terminal state.

## Out of Scope

Node instances beyond Described, and what each run transition does to them (`story:node-instance-lifecycle`); edges carrying events (`story:bounded-edges`); the text pipeline (`story:text-pipeline-end-to-end`); resource admission outcomes `not_enough_vram`, `backend_busy`, `deadline_unachievable` (`docs/design/inference.md:693`, no noun yet).

## Ambiguities

- `inferable`: a run of an unplanned pipeline. The runner creates it because graph.yaml declares no other outcome; the library never does because its entry point takes the Planned form only.

## Open Questions

None.

## Scenarios

The 66 `inference.graph` scenarios this story answers, read from `.engineering/drafts/suite.json` (synthesized 2026-10-18):

- `inference.graph.ActivateRun/outcome/active`
- `inference.graph.ActivateRun/outcome/wrong-state`
- `inference.graph.CancelRun/outcome/cancelled`
- `inference.graph.CancelRun/outcome/wrong-state`
- `inference.graph.CompleteRun/outcome/completed`
- `inference.graph.CompleteRun/outcome/wrong-state`
- `inference.graph.CreateRun/outcome/created`
- `inference.graph.DescribeNodeInstance/outcome/described`
- `inference.graph.DrainRun/outcome/draining`
- `inference.graph.DrainRun/outcome/wrong-state`
- `inference.graph.FailRun/outcome/failed`
- `inference.graph.FailRun/outcome/wrong-state`
- `inference.graph.PrepareRun/outcome/preparing`
- `inference.graph.PrepareRun/outcome/wrong-state`
- `inference.graph.Run/state/Active/refuses/inference.graph.ActivateRun`
- `inference.graph.Run/state/Active/refuses/inference.graph.CompleteRun`
- `inference.graph.Run/state/Active/refuses/inference.graph.PrepareRun`
- `inference.graph.Run/state/Active/refuses/inference.graph.ValidateRun`
- `inference.graph.Run/state/Cancelled/refuses/inference.graph.ActivateRun`
- `inference.graph.Run/state/Cancelled/refuses/inference.graph.CancelRun`
- `inference.graph.Run/state/Cancelled/refuses/inference.graph.CompleteRun`
- `inference.graph.Run/state/Cancelled/refuses/inference.graph.DrainRun`
- `inference.graph.Run/state/Cancelled/refuses/inference.graph.FailRun`
- `inference.graph.Run/state/Cancelled/refuses/inference.graph.PrepareRun`
- `inference.graph.Run/state/Cancelled/refuses/inference.graph.ValidateRun`
- `inference.graph.Run/state/Completed/refuses/inference.graph.ActivateRun`
- `inference.graph.Run/state/Completed/refuses/inference.graph.CancelRun`
- `inference.graph.Run/state/Completed/refuses/inference.graph.CompleteRun`
- `inference.graph.Run/state/Completed/refuses/inference.graph.DrainRun`
- `inference.graph.Run/state/Completed/refuses/inference.graph.FailRun`
- `inference.graph.Run/state/Completed/refuses/inference.graph.PrepareRun`
- `inference.graph.Run/state/Completed/refuses/inference.graph.ValidateRun`
- `inference.graph.Run/state/Created/refuses/inference.graph.ActivateRun`
- `inference.graph.Run/state/Created/refuses/inference.graph.CompleteRun`
- `inference.graph.Run/state/Created/refuses/inference.graph.DrainRun`
- `inference.graph.Run/state/Created/refuses/inference.graph.FailRun`
- `inference.graph.Run/state/Created/refuses/inference.graph.PrepareRun`
- `inference.graph.Run/state/Draining/refuses/inference.graph.ActivateRun`
- `inference.graph.Run/state/Draining/refuses/inference.graph.DrainRun`
- `inference.graph.Run/state/Draining/refuses/inference.graph.FailRun`
- `inference.graph.Run/state/Draining/refuses/inference.graph.PrepareRun`
- `inference.graph.Run/state/Draining/refuses/inference.graph.ValidateRun`
- `inference.graph.Run/state/Failed/refuses/inference.graph.ActivateRun`
- `inference.graph.Run/state/Failed/refuses/inference.graph.CancelRun`
- `inference.graph.Run/state/Failed/refuses/inference.graph.CompleteRun`
- `inference.graph.Run/state/Failed/refuses/inference.graph.DrainRun`
- `inference.graph.Run/state/Failed/refuses/inference.graph.FailRun`
- `inference.graph.Run/state/Failed/refuses/inference.graph.PrepareRun`
- `inference.graph.Run/state/Failed/refuses/inference.graph.ValidateRun`
- `inference.graph.Run/state/Preparing/refuses/inference.graph.CompleteRun`
- `inference.graph.Run/state/Preparing/refuses/inference.graph.DrainRun`
- `inference.graph.Run/state/Preparing/refuses/inference.graph.PrepareRun`
- `inference.graph.Run/state/Preparing/refuses/inference.graph.ValidateRun`
- `inference.graph.Run/state/Validating/refuses/inference.graph.ActivateRun`
- `inference.graph.Run/state/Validating/refuses/inference.graph.CompleteRun`
- `inference.graph.Run/state/Validating/refuses/inference.graph.DrainRun`
- `inference.graph.Run/state/Validating/refuses/inference.graph.ValidateRun`
- `inference.graph.Run/transition/activate/by/inference.graph.ActivateRun/active`
- `inference.graph.Run/transition/cancel/by/inference.graph.CancelRun/cancelled`
- `inference.graph.Run/transition/complete/by/inference.graph.CompleteRun/completed`
- `inference.graph.Run/transition/drain/by/inference.graph.DrainRun/draining`
- `inference.graph.Run/transition/fail/by/inference.graph.FailRun/failed`
- `inference.graph.Run/transition/prepare/by/inference.graph.PrepareRun/preparing`
- `inference.graph.Run/transition/validate/by/inference.graph.ValidateRun/validating`
- `inference.graph.ValidateRun/outcome/validating`
- `inference.graph.ValidateRun/outcome/wrong-state`

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). No code and no commits exist in the tree; every path
below is one the story, a `depends_on` story or `story:testkit-conformance-target` creates.

- **Primary surface:** `crates/inference-runtime` — cited (Context: "adds the Run state machine to `crates/inference-runtime`"; `docs/design/inference.md:242`); the crate, its `Cargo.toml` and `src/lib.rs` are `story:bounded-edges` step 2's, and this story's work there is one new module and one line
- **Files:** `crates/inference-runtime/src/lib.rs` — cited (Context: "one line to `src/lib.rs`"); the `mod` line for the new module, in a file another story creates
- **Files:** `crates/inference-testkit/tests/target/graph.rs` — cited (step 4: "the runner answers the nine commands and the views"; the graph-domain module of the conformance target is that file, `story:testkit-conformance-target` step 2, which step 4 points at); the eight Run commands, `DescribeNodeInstance`, `inference.graph.Runs` with `state`, `inference.graph.NodeInstances`
- **Files:** `ess/SKIPPED.md` — cited (step 4, Acceptance); absent in the tree today, created with one `## graph` section by `story:testkit-conformance-target` step 3; this story removes exactly the 66 ids under Scenarios, all 66 present as keys of `.engineering/drafts/suite.json` (153 scenarios, `scenario_initial_state: empty`)
- **Symbols:** `inference.graph.Run` and its transition table (`ess/domains/graph.yaml:284-333`, identical to step 3), `inference.graph.NodeInstance` with the one state Described (336-357), `inference.graph.RunStateConflict { current }` (380-384), `CreateRun` 621, `ValidateRun` 639, `PrepareRun` 660, `ActivateRun` 681, `DrainRun` 702, `CompleteRun` 723, `FailRun` 746, `CancelRun` 772, `DescribeNodeInstance` 793, views `Runs` 1002 and `NodeInstances` 1013; the Rust names `Run`, `RunId`, the state enum, `RunStateConflict`, `NodeInstance`, `NodeInstanceId` (step 2) exist in no file yet — cited
- **Also likely:** `crates/inference-runtime/src/run.rs` — inferred: step 2 says "a module of its own beside the queue module" and names neither file; the queue module is recorded as `src/queue.rs` by the same guess
- **Also likely:** `crates/inference-testkit/src/graph.rs` — inferred: the graph store module the target calls (`story:testkit-conformance-target` step 1, "one store module per domain"); the Run and NodeInstance stores and the two views land here, the file name follows the `src/<domain>.rs` convention of that story's own scope entry, itself inferred
- **Tests:** step 5's Rust test (Planned form to Completed; `CancelRun` from each non-terminal state) lands under `crates/inference-runtime`, inline or under `tests/`; the body does not say — inferred, covered by the primary-surface entry
- **Read, not changed:** `crates/inference-pipeline` (the Planned form, `story:capability-planning` step 2, the only constructor input), `crates/inference-runtime/Cargo.toml` (`story:bounded-edges` step 2 already declares `inference-pipeline` and `inference-core`; the body names no further dependency), `ess/domains/graph.yaml` (step 1: nothing to add), `docs/design/inference.md` (all nine cited lines, 242, 519, 641, 651, 665, 671, 673, 693, 1006, say what the body says they say) — cited
- **Documents:** `ess/SKIPPED.md` only, 66 lines removed from its `## graph` section; `AGENTS.md`, `Taskfile.yml`, `README.md` untouched, the body names none of them — cited
- **Confidence:** high — the body names the crate, the `lib.rs` line, the testkit runner and `ess/SKIPPED.md`; every symbol resolves to a `graph.yaml` line and all 66 scenario ids to a suite key; only the two file names inside the crates and the test's location are inferred
- **Would collide with:** any unit writing the graph-domain files of the testkit (`crates/inference-testkit/tests/target/graph.rs`, `crates/inference-testkit/src/graph.rs`), which every `inference.graph` story's testkit step does; any unit editing the `## graph` section of `ess/SKIPPED.md`; any unit adding a `mod` line to `crates/inference-runtime/src/lib.rs` or otherwise writing under `crates/inference-runtime`
- **Safety fact:** the 66 scenarios are closed over the Run and NodeInstance commands and the two views: each starts from an empty state, creates its run with `CreateRun` on a literal `pipeline_id` no pipeline was declared for, and `CreateRun` has one outcome and no guard (`ess/domains/graph.yaml:621-638`), so the runner's Run store needs no pipeline lookup and no other command branch of `tests/target/graph.rs` changes; a refusal asserts `RunStateConflict.current` equals the state it was in (`fields: {current: "Created"}`) and `expect_no_event` for every other graph event, so a refused command that moves and emits nothing satisfies all 45 refusal ids — step 3 (walked `Run/state/Created/refuses/PrepareRun`, `PrepareRun/outcome/wrong-state`, `DescribeNodeInstance/outcome/described`, `Run/transition/cancel/.../cancelled` in `.engineering/drafts/suite.json`), unproven: no code exists. Where the symbol search stops: the wire format, `Run.State` variants PascalCase (`"Created"`), outcomes kebab-case (`wrong-state`), `RunId`, `PipelineId`, `NodeInstanceId` as Uuid newtypes, and `story:node-instance-lifecycle` step 1 re-synthesizes these 66 with more `expect_no_event` lines (its line 36 claims they keep passing) — step 2, unproven
