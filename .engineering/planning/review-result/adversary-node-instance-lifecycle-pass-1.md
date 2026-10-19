---
format: aep.planning-md/3
id: review-result:adversary-node-instance-lifecycle-pass-1
kind: review-result
status: active
title: Adversary, story:node-instance-lifecycle, pass 1
relations:
- reviews: story:node-instance-lifecycle
revision: 1
---
```
unit: story:node-instance-lifecycle, working tree of impl/node-instance-lifecycle at base 010843f with the change uncommitted
verdict: CONFIRMED
cases: executed 38→49 (inference-runtime), red 2
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 13 paths, all under scratch/adversary-1/ (part 6)
needs-coordinator: none
```

**Result:** the controller breaks after a caller abandons one of its futures (a timeout or `select!`): `activate()` panics in one case, and `complete()` ends a run Completed with Started instances in the other. Everything else in the focus list held. No caller in the tree does this; it is reachable through the public API only.

## 1. Diff
`git --no-pager diff --stat` is the unit's own: 11 files, 1152 insertions, 41 deletions, the same as at hand-off. I added two untracked test files and nothing else:
- `crates/inference-runtime/tests/adversary1_node_instance_table.rs`
- `crates/inference-runtime/tests/adversary1_node_instance_controller.rs`

No non-test path was touched.

## 2. Cases added (red output from the first run of these files alone, before the suite ran)

| Case | Asserts | Now |
|---|---|---|
| table: `every_state_answers_every_command_as_graph_yaml_declares` | all 7 states × 6 commands against the graph.yaml transitions; a refusal reports `current` and changes nothing | green |
| table: `the_terminal_states_are_stopped_and_failed_only` | only Stopped and Failed are terminal | green |
| table: `the_states_and_the_conflict_serialize_by_their_ess_names` | ESS order and names; `{"current":..}`; `NodeInstanceFailed` has exactly 2 fields | green |
| controller: cancel while Preparing, cancel while Draining, drain error, `fail_node` while Draining, caller `fail()` while Active, `fail_node` before describe | call sequences and final states | green |
| controller: `activating_after_an_abandoned_prepare_does_not_panic` (:369) | after `prepare()` is polled once and dropped, `activate()` answers instead of panicking | **red** |
| controller: `completing_after_an_abandoned_drain_leaves_no_instance_unstopped` (:392) | after `drain()` is dropped, `complete()` either refuses or leaves every instance terminal | **red** |

Red output, verbatim:
```
thread 'activating_after_an_abandoned_prepare_does_not_panic' panicked at crates/inference-runtime/src/controller.rs:401:26:
the controller drives each instance along its lifecycle: NodeInstanceStateConflict { current: Described }
---- completing_after_an_abandoned_drain_leaves_no_instance_unstopped stdout ----
CompleteRun answered Ok(()) and the run is Completed while its instances are [Some(Started), Some(Started)]
```

## 3. Suite and gate (run after the cases existed; logs in scratch)

| Command | Exit | Result |
|---|---|---|
| `cargo test -p inference-runtime --all-features --no-fail-fast` | 101 | 47 passed, 2 failed (the two above) |
| `cargo clippy -p inference-runtime --all-targets --all-features -- -D warnings` | 0 | clean |
| `cargo fmt -p inference-runtime --check` | 0 | clean, after `rustfmt` on my two files only |
| `cargo test -p inference-testkit --all-features --no-fail-fast` | 0 | all ok |
| conform | 0 | `counts: {"error":0,"failed":0,"passed":243,"skipped":0,"total":277,"unsupported":34}`; the test checks unsupported == SKIPPED.md |
| `cargo test -p stt-only --test tree` | 0 | 1 passed |

`cargo tree -p stt-only` names no `inference-runtime`. Disk was at 9.7G free for the incremental gate runs; the mutant build started at 11G.

## 4. Findings
- **F1** `crates/inference-runtime/src/controller.rs:401`. In `answered`, an `expect` assumes every instance is in the step's source state. If the caller drops `prepare()`, the run stays Preparing with Described instances, and `activate()` then panics. Dropping `activate()` and then calling `drain()` reaches the same `expect`; that one is inferred, not run. CONFIRMED, introduced.
  - Measured: red case at `adversary1_node_instance_controller.rs:369`.
  - What reaches it: no caller in the tree; any executor timeout or `select!` around `prepare()`.
  - Fix to name: check each instance's state before the run moves and answer a conflict, or document that the methods are not cancel-safe.
- **F2** `crates/inference-runtime/src/controller.rs:296` (`complete`) and `:445` (`stop`). `stop` ignores the refusal it gets from a Started instance. After a dropped `drain()`, `complete()` returns `Ok`, the run is Completed, and no node ever gets `stop`. This contradicts CompleteRun ("released its worker resources"). CONFIRMED, introduced.
  - Measured: red case at `:392`.
  - What reaches it: same as F1.
- **Doc patches** checked against the code: `endpoint.rs` matches (one `EdgeQueue` per edge in `create`, endpoints handed out in `activate`), and so does testkit `lib.rs` (`RecordingNode` sits behind `run` in `src/graph.rs`). No finding.

## 5. Attacked and held
- **ESS against Rust:** the 6 commands, `NodeInstanceStateConflict { current }`, the 6 events (`reason: String`), state order, terminal states and the view's `state` all match.
- **Refusals:** move nothing and emit nothing, in Rust (immutable `moved`) and in the testkit (`expect_no_events`, `expect_complete_subject_unchanged`).
- **Mutants:** I removed Draining from `fail`, Draining from `cancel`, and Prepared from `cancel`. Conform caught all three (242 passed, 1 failed each); the unit's runtime tests missed all three; my cases caught all three.
- **Run-to-instance coupling:** order, `cancel` after `start`, `fail_node`, and no further calls to a Failed instance all behave as specified.
- **The 6 `wrong-state` outcome scenarios** pass through the unknown-instance path, without `current`. That is how ESS synthesizes them; the 28 `refuses` scenarios check `current`.
- **Dev-dependency cycle runtime ↔ testkit** builds, and the integration tests link a single copy of the runtime.

## 6. Paths written outside the worktree
All under `<unit scratch>/adversary-1/`:
- Logs: `clippy.log`, `conform.log`, `fmt.log`, `gate-runtime.log`, `gate-testkit.log`, `table.log`, `mutant-cancel-from-Draining.log`, `mutant-cancel-from-Prepared.log`, `mutant-fail-from-Draining.log`, `mutant-fail-detail.log`, `mutants.done`
- Scripts: `mutants.sh`, `show.py`

The mutant copy (`mutant/`, with its 744M `target/`), `run.rs.orig` and `suite.json` are deleted. The lease is released.

```findings
- file: crates/inference-runtime/src/controller.rs
  line: 401
  category: concurrency
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: after a caller abandons a prepare() future, activate() panics on the lifecycle expect instead of answering, because the run moved while its instances stayed Described
- file: crates/inference-runtime/src/controller.rs
  line: 296
  category: concurrency
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: after a caller abandons a drain() future, complete() answers Ok and leaves the run Completed with Started instances that never receive stop, because stop() ignores the refusal
```