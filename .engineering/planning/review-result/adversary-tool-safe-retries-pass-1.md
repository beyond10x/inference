---
format: aep.planning-md/3
id: review-result:adversary-tool-safe-retries-pass-1
kind: review-result
status: active
title: Adversary, story:tool-safe-retries, pass 1
relations:
- reviews: story:tool-safe-retries
revision: 1
---
unit: story:tool-safe-retries, working tree of inference-impl-tool-safe-retries (base cf6bc47 plus the unit's uncommitted change)
verdict: CONFIRMED
cases: executed 94→101, red 0 on the tree (5 red against mutants)
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory (part 6)
needs-coordinator: none

Nothing in the shipped behaviour broke. But five guards in the new retry loop are untested by the unit's 6 cases and by the wave-14 tests: delete any one of them and every one of those tests stays green. My new file catches each of them.

**1. Diff.** `git --no-pager diff --stat` shows only the implementor's 4 files (`tool_executor.rs`, `mock_tool_executor.rs`, `inference-tools/src/executor.rs`, `inference-tools/src/lib.rs`, 275+/49-). My only path is the untracked test file `crates/inference-runtime/tests/adversary1_tool_retries.rs`. I changed no implementation file.

**2. Cases added** (all in that file, all green on the tree now). Each red line below is the output from one mutated scratch copy. The mutants were never applied in the worktree.

| Case | Asserts | Red on mutant |
|---|---|---|
| `adversary1_a_failure_other_than_an_execution_error_is_not_attempted_again` | an executor answering `deadline_exceeded` or `cancelled` gets 1 attempt under `up_to(3)` | M1 `retries()` treated as always true: `:414 DeadlineExceeded is attempted once left: 2 right: 1` |
| `adversary1_no_attempt_starts_after_the_run_is_cancelled` | executor turns the cancel into `execution_error`; 1 attempt, nothing sent after Started | M2 cancel check removed: `:563 nothing is attempted after the cancel left: 2 right: 1` |
| `adversary1_no_attempt_starts_after_the_shared_deadline` | 60ms deadline, first attempt takes 120ms; no attempt starts after the deadline | M3 deadline check removed: `:527 no attempt started past the deadline: [Attempt {…}]` |
| `adversary1_the_measure_covers_every_attempt` | 3 attempts, latency ≥ 80ms across two 40ms failures | M4 latency set to `Duration::ZERO`: `:496 two attempts of 40ms each lie inside the call's latency: 0ns` |
| `adversary1_every_attempt_is_given_the_one_deadline_the_call_started_with` | same deadline `Instant`, call_id and generation_id on all 3 attempts | M5 deadline recomputed per attempt: `:451 one deadline for every attempt: [Attempt {…}]` |
| `adversary1_a_non_idempotent_write_is_attempted_once_under_the_largest_limit` | `up_to(u32::MAX)`, always failing: 1 attempt, Failed, measure attempts 1 | none |
| `adversary1_two_calls_of_one_flaky_tool_interleaved_with_their_redeliveries` | deliveries A,B,A,B: invocations A,A,B,B; 2 Completed refusals; attempts [2,2] | none |

On mutants M1 to M5, `tool_retries` (6), `tool_nodes` (8) and `adversary1_tool_nodes` (9) all passed with exit 0. On the unmutated copy (M0) all 4 targets passed. Full log: `scratch/adversary-1/mutants.log`.

**3. Gate** (run after the cases existed):
- `cargo test -p inference-runtime --all-features --no-fail-fast`: exit 0, 101 passed, 0 failed. My file accounts for 7, which gives the 94 "before" by subtraction, not a separate run.
- `cargo clippy -p inference-runtime --all-targets --all-features -- -D warnings`: exit 0.
- `cargo fmt -p inference-runtime --check`: exit 0. A first run failed with exit 1 on my own file only; I ran `rustfmt` on that file and reran.
- Deviation: in my first gate attempt, `set -x` under zsh did not export the debug-off variables, so that clippy build used debuginfo. Disk went from 812G to 813G used, and the extra artifacts are still in the worktree's `target/`, which I did not delete.

**4. Findings** (covering the working tree above):

| file:line | Finding | Verdict / origin | What reaches it |
|---|---|---|---|
| tool_executor.rs:303 | only-`execution_error` guard (M1) untested | CONFIRMED / introduced | any executor that answers its own `deadline_exceeded` or `cancelled` |
| tool_executor.rs:304 | no-retry-after-cancel guard (M2) untested | CONFIRMED / introduced | run cancel during an attempt, with an executor that reports the abort as `execution_error` |
| tool_executor.rs:305 | shared-deadline guard (M3) untested | CONFIRMED / introduced | an attempt that answers after the call's deadline |
| tool_executor.rs:347 | measure latency (M4) unasserted | CONFIRMED / introduced | every measured call |
| tool_executor.rs:291 | one deadline across attempts (M5) unasserted | CONFIRMED / introduced | every retried call |

All 5 are closed once my file merges. No implementation fix is needed.

**5. Attacked and not broken:**
- A non-idempotent write is never retried, under limit 0, 3, 5 or `u32::MAX`, at both the policy and the node.
- `up_to(0)` and the default policy give 1 attempt, with no u32 overflow.
- Redelivery of a Completed or Failed call, and interleaved with another call: exactly one `wrong-state` refusal each, and the run goes on.
- The mock counts per call_id across two calls of one tool.
- call_id and generation_id stay the same across attempts, with one log entry per attempt.
- The measure has no off-by-one in attempts, and its failure is None on success.
- A Started-state redelivery cannot happen at the node: the node handles calls one at a time and ends after a cancel.
- `tool_nodes` and `adversary1_tool_nodes` are still green.

**6. Written outside the worktree:** `<unit scratch>/adversary-1/`. It holds `mutate.sh`, `mutants.log`, 24 `out-M*.txt` files and `gate-{test,clippy,fmt}.txt`. The mutant copy and its target (259M) were deleted after the run.

**7.**
```findings
- file: crates/inference-pipeline/src/nodes/tool_executor.rs
  line: 303
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: retrying every failure kind survives the unit's suite; only adversary1_a_failure_other_than_an_execution_error_is_not_attempted_again catches it
- file: crates/inference-pipeline/src/nodes/tool_executor.rs
  line: 304
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: dropping the cancellation check lets a cancelled call be attempted again with the unit's suite green
- file: crates/inference-pipeline/src/nodes/tool_executor.rs
  line: 305
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: dropping the shared-deadline check lets an attempt start past the deadline with the unit's suite green
- file: crates/inference-pipeline/src/nodes/tool_executor.rs
  line: 347
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a zero latency on ToolCallMeasure survives the unit's suite, which never asserts latency
- file: crates/inference-pipeline/src/nodes/tool_executor.rs
  line: 291
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: giving each attempt a fresh deadline survives the unit's suite, contradicting the one-deadline claim
```