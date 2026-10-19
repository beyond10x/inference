---
format: aep.planning-md/3
id: review-result:adversary-run-lifecycle-pass-1
kind: review-result
status: active
title: Adversary, story:run-lifecycle, pass 1
relations:
- reviews: story:run-lifecycle
revision: 1
---
unit: story:run-lifecycle, uncommitted worktree inference-impl-run-lifecycle on base 1111057 (implementor change plus the coordinator's three patches)
verdict: CONFIRMED (one test gap; the code is correct, so no case is red)
cases: executed 502→547 (testkit 470→515, runtime 32→32), red 0
origin: introduced 1 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 path (the scratch directory)
needs-coordinator: none

I could not make the run-lifecycle change fail. I found one gap: a refused `CancelRun` that wrongly cancels generations would pass every existing test and conform. My new guard test catches it.

**1. Diff.** `git diff --stat 1111057` is unchanged at 15 files, +2197 −310. I added one untracked file, `crates/inference-testkit/tests/adversary1_run_target.rs` (+394). It is a test file. I changed no implementation file.

**2. Cases added** (all in that file; all green now, alone and after formatting: 45 passed, exit 0)

| line | case | what it checks |
|---|---|---|
| 215 | `a_cancel_run_refused_from_a_terminal_state_touches_none_of_its_generations_or_segments` | A run is Completed or Failed but still owns an Active generation and a Tentative segment. `CancelRun` must answer `wrong-state` with `current` set, emit no event, and leave the Generations and Segments views unchanged. |
| 259 | `cancel_run_from_draining_cancels_the_runs_active_generation_and_discards_its_segment` | The cancel cascade also works from Draining. The authored scenario only cancels from Active. |
| 335 | `every_run_command_from_every_state_answers_as_the_transition_table_declares` | All 8 states × 7 commands go through the target: 14 moves (each emits one event naming the run) and 42 refusals (correct `current`, no event, Runs row unchanged). |

**3. Gate, run after the cases existed** (each command on its own, worktree)

| command | exit | result |
|---|---|---|
| `cargo test -p inference-testkit --all-features --no-fail-fast` | 0 | 515 passed, 0 failed |
| `cargo test -p inference-runtime --all-features --no-fail-fast` | 0 | 32 passed, 0 failed |
| `cargo clippy -p inference-testkit --all-targets --all-features -- -D warnings` | 0 | clean |
| `cargo fmt -p inference-testkit --check` | 1, then 0 | only my file needed formatting; I ran `rustfmt` on it and re-ran the case and clippy (both exit 0) |
| `cargo fmt -p inference-runtime --check` | 0 | clean |
| `cargo test -p inference-testkit --test conform` | 0 | `counts: {"error":0,"failed":0,"passed":197,"skipped":0,"total":231,"unsupported":34}`; the 34 unsupported match the 34 ids in `ess/SKIPPED.md` |

The "before" count is the "after" count minus my test binary, which executes 45 cases: my 3, plus 42 module tests that `mod target;` runs again.

**4. Findings**

- **Refused `CancelRun` cascade is unguarded** (`crates/inference-testkit/tests/target/graph.rs:357`; CONFIRMED, origin introduced, severity warning).
  - **Measured:** I removed the `if answer.is_ok()` check so `stream.cancel_run` runs even when `CancelRun` is refused. Conform stayed green and so did every test the unit wrote. Only my case at line 215 failed.
  - **What reaches it:** an ordinary sequence: Activate, StartGeneration, then Drain and Complete (or Fail), then `CancelRun`. Neither Complete nor Fail touches generations (graph.yaml leaves that open), so the run still owns live work when the cancel is refused. Today's code is correct; nothing guarded it until this case.
  - **Fix:** keep the line-215 case.

- **Unknown run on a moving command** (`target/graph.rs:280`; judgement, note, origin introduced). A moving Run command naming a run that does not exist answers `wrong-state` with a `RunStateConflict` that has no `current` field, the same way `ValidatePipeline` handles an unknown pipeline. graph.yaml declares no `unknown-run` outcome for these commands. No scenario reaches this.

**Implementor decisions I checked**

- **Refusal order (unknown identity before state): confirmed against the specification.** I wrote probe scenarios in scratch and ran them on the interpreted target. A Created run with an unknown node answers `unknown-node` (passed); the `run-not-active` variant failed.
- **Forced `PrepareRun/refused`** answering any state, including an unknown run: the synthesized `PrepareRun/outcome/refused` scenario forces it on a run that does not exist, so the target has to answer this way.
- **Run ids in group `00000003-0001-…`, `GraphDomain` no longer `Clone`, and `plan_of`'s `unreachable!`:** the only path to Validated stores the validated form, so the `unreachable!` branch cannot be hit.

**5. What I attacked and could not break.** I ran 18 mutants on a scratch copy. Besides the one above:
- **Caught by conform:** fail not allowed from Active, cascade removed, not-planned answered as unknown-pipeline, describe and start checking state before node, failure missing from the Runs view, cancel allowed from Completed, FailRun dropping `node_id`, cascade skipping Committed or Queued/Synthesized segments, cancel not allowed from Draining, fail not recording the failure.
- **Missed by conform, caught by the unit's tests:** describing a node instance while Validating, a forced refusal that still moves the run, and a cascade that hits every run's generations or segments ("only that run's").
- **Not mutated, checked by reading:** `Run::create_as` takes only `&PlannedPipeline`, which has private fields, no `Deserialize`, and is built only by `Declaration::plan`. Rust field order and names match the ESS declarations.

**6. Written outside the worktree.** One directory: `<unit scratch>/adversary-1/` (suite.json, probe scenarios and report, mutant driver, mutant logs, gate logs). I deleted the mutant workspace copy and its `target/` before reporting. Lease released (session-end exit 0).

```findings
- file: crates/inference-testkit/tests/target/graph.rs
  line: 357
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: running the CancelRun stream cascade on a refused CancelRun survives conform and every unit-written test; only adversary1_run_target.rs:215 catches it
- file: crates/inference-testkit/tests/target/graph.rs
  line: 280
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a moving Run command naming an unknown run answers wrong-state with a RunStateConflict missing its declared current field, an outcome graph.yaml does not declare for an absent run
```