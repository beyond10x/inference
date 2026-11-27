---
format: aep.planning-md/3
id: review-result:test-run-step-under-budget-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the test-run step under budget
relations:
- reviews: story:test-run-step-under-budget
revision: 1
---
Adversary pass 1 on story:test-run-step-under-budget, 2026-11-24, as the adversary returned it. The coordinator replaced two path prefixes outside the repository with `<worktree>` and `<scratch>` (the repository is public); nothing else is changed.

```
unit: story:test-run-step-under-budget, commits b81b9b08f + 34e8ac136 (head 34e8ac136d8e676bc336fe229a362ab013e93483, base 0fe11ff08)
verdict: red
cases: executed 1394→1400, red 5
origin: introduced 6, pre-existing 0, undecided 0
wrote-outside-worktree: 9 files under <scratch> (part 6)
needs-coordinator: yes
```

Needs the coordinator: (a) whether acceptance line 2 ("calls synthesis once") admits three syntheses per cold run (F3); (b) acceptance line 3 (load above 120) and line 1 (CI wall time) were not attacked: no load test allowed, and no measured run was handed to me.

## 1. `git --no-pager diff --stat`

Empty (no tracked file changed). `git status --short`:

```
?? crates/inference-testkit/tests/adversary_w1_test_budget.rs
```

One untracked test file, uncommitted. No implementation file touched.

## 2. Cases added

File: `<worktree>/crates/inference-testkit/tests/adversary_w1_test_budget.rs`

| case | asserts | now |
|---|---|---|
| `adversary_w1_the_test_run_task_exits_124_when_its_bound_ends_it` | the `test-run` command line copied from `Taskfile.yml` (bound cut to 1 s, parts sleep) prints the line, prints the slowest header, runs no later part, and `task` exits 124 as the guide says | red: exit 201 |
| `adversary_w1_the_test_run_task_names_its_bound_when_a_part_outlives_the_first_signal` | same line, a part that ignores SIGTERM: the bound line is still printed | red: no line, 137 |
| `adversary_w1_a_run_of_the_testkits_tests_calls_synthesis_once` | reads the three suites the crate's tests read, counts `synthesized.log` lines of their keys: 1 call | red: 3 (13.0 s on a cold cache, 0.09 s warm) |
| `adversary_w1_an_empty_suite_file_is_not_served_as_the_suite` | an emptied `suite.json` is synthesized again | red: `""` served |
| `adversary_w1_planted_this_test_synthesizes_into_a_directory_of_its_own` | planted defect for the next case: real `ess` through `suite::Cache` + `suite::Ess` into a private directory | green by design (4.4 s) |
| `adversary_w1_no_test_names_the_ess_synthesizer_but_the_shared_cache` | the unit's scan, rewritten to also see `suite::Ess` | red: names the planted file |

First run, the file alone (`CARGO_INCREMENTAL=0 cargo nextest run -p inference-testkit --all-features --test adversary_w1_test_budget --no-fail-fast`, exit 100), panics verbatim:

```
panicked at crates/inference-testkit/tests/adversary_w1_test_budget.rs:85:5:
assertion `left == right` failed: the guide says `task test-run` exits 124 when its bound ends it:
exit status: Some(201)
stdout:
task: Signal received: "terminated"
the ten slowest tests, in seconds:
task test-run was ended at its bound of 120 s

stderr:
task: [test-run] timeout --kill-after=1s 1s task test-run-parts || { status=$?; if [ "$status" -eq 124 ]; then echo "task test-run was ended at its bound of 120 s"; fi; exit "$status"; }
task: [test-workspace] sleep 30
task: [test-slowest] echo "the ten slowest tests, in seconds:"
task: Failed to run task "test-run-parts": task: Failed to run task "test-workspace": exit status 143
task: Failed to run task "test-run": exit status 124

  left: Some(201)
 right: Some(124)
```

```
panicked at crates/inference-testkit/tests/adversary_w1_test_budget.rs:98:5:
the line of the bound is printed when the kill ends the step:
exit status: Some(201)
stdout:
task: Signal received: "terminated"

stderr:
task: [test-run] timeout --kill-after=1s 1s task test-run-parts || { ... }
task: [test-workspace] sh -c 'trap "" TERM; exec sleep 30'
task: Failed to run task "test-run": exit status 137
```

```
panicked at crates/inference-testkit/tests/adversary_w1_test_budget.rs:151:5:
assertion `left == right` failed: the suites the tests of this crate read were made by this many calls of synthesis, one per key: [
    "07f2c965a1a4dc86952280bb4a6708382b6d00e9eda1f2947c636e8d4be40240: 1 line(s)",
    "de961aeaadc6d5a16bd9aeeea637aa5de06702beadc344984a84b3a55d30f11b: 1 line(s)",
    "c6f3abf23885511daed16f1afafa3704031519d1439aed26fef643075a117721: 1 line(s)",
]
  left: 3
 right: 1
```

```
panicked at crates/inference-testkit/tests/adversary_w1_test_budget.rs:194:5:
assertion `left == right` failed: an empty suite.json is synthesized again, not served
  left: ""
 right: "the whole suite\n"
```

```
panicked at crates/inference-testkit/tests/adversary_w1_test_budget.rs:263:5:
these files run synthesis outside the shared cache: [
    "adversary_w1_test_budget.rs",
]
```

Summary of that run: `6 tests run: 1 passed, 5 failed, 0 skipped` in 13.0 s.

## 3. Suite run (after the cases existed)

`CARGO_INCREMENTAL=0 cargo nextest run -p inference-testkit --all-features --no-fail-fast`, exit 100:

```
Starting 1400 tests across 68 binaries
Summary [   6.483s] 1400 tests run: 1395 passed, 5 failed, 0 skipped
FAIL [   0.006s] inference-testkit::adversary_w1_test_budget adversary_w1_an_empty_suite_file_is_not_served_as_the_suite
FAIL [   0.007s] inference-testkit::adversary_w1_test_budget adversary_w1_no_test_names_the_ess_synthesizer_but_the_shared_cache
FAIL [   0.090s] inference-testkit::adversary_w1_test_budget adversary_w1_a_run_of_the_testkits_tests_calls_synthesis_once
FAIL [   1.081s] inference-testkit::adversary_w1_test_budget adversary_w1_the_test_run_task_exits_124_when_its_bound_ends_it
FAIL [   2.068s] inference-testkit::adversary_w1_test_budget adversary_w1_the_test_run_task_names_its_bound_when_a_part_outlives_the_first_signal
error: test run failed
```

In that run the unit's `no_test_of_this_crate_synthesizes_outside_the_shared_cache` and `every_suite_of_this_repository_was_synthesized_at_most_once` passed with the planted file present.

`<before>`: the same command with `-E 'not binary(adversary_w1_test_budget)'`, run afterwards: `1394 tests run: 1394 passed, 0 skipped` in 6.4 s, exit 0. Slowest tests about 5.6 s; none near 60 s. No compiler warning in the testkit build.

## 4. Findings (cover head 34e8ac136)

| # | file:line | category | verdict | origin | severity | measured | what reaches it |
|---|---|---|---|---|---|---|---|
| F1 | `website/docs/guides/run-the-checks.md:41` | contract-drift | NEEDS-CHANGE | introduced | warning | `task test-run` exits 201 when the bound ends it, not 124; 124 is only `timeout`'s status inside the command line (go-task 3.54.0 on the node and the host; CI pins 3.52.0, not run there). The same claim is in the message of 34e8ac136. The bound line and the slowest header are printed | CI's step `run: task test-run` (`.github/workflows/check.yml:145`) and `task check`. The step is red either way; the published sentence is false. `task -x` returned 124 in a local probe |
| F2 | `Taskfile.yml:35` | boundary | INFEASIBLE | introduced | note | a part still alive at `--kill-after`: `timeout` (coreutils 9.12) kills its own group with SIGKILL, itself included, the shell sees 137, the `-eq 124` test misses: no bound line, no slowest list | nothing found: it needs a part that outlives SIGTERM by 10 s. No test in the tree handles SIGTERM. cargo-nextest's default grace period is also 10 s (its documentation, not measured) |
| F3 | `crates/inference-testkit/tests/suite/mod.rs:212` | acceptance | CONFIRMED | introduced | warning | a cold cache takes 3 calls of synthesis (plain, plus one per witness file: `adversary12_duplex_close_witness.rs:278`, `adversary_w23_descriptor_view.rs:219`), 13.0 s in series; acceptance says "calls synthesis once" | every CI run on a code pull request (fresh `target/`), and the first local run after a change under `ess/` or of the `ess` version. Down from 13; whether three meets the line is the coordinator's |
| F4 | `crates/inference-testkit/tests/suite/mod.rs:95` | boundary | INFEASIBLE | introduced | note | an empty `suite.json` is returned as the suite, never synthesized again; the module comment at `:14` says a file that is there is whole because a rename is atomic | nothing found short of a machine crash between the rename and the write-back. The result is red tests ("the synthesized suite is admitted") until the key directory is removed by hand, not a false green |
| F5 | `crates/inference-testkit/tests/conformance_suite.rs:255` | mutant | CONFIRMED | introduced | note | the scan matches one literal; `suite::Ess` and `suite::Cache` are `pub`, so a test synthesizes per test into a directory of its own and the scan and the log test stay green (planted case, real `ess`, passed in the suite run). A unique `Added.yaml` per test steps around it the same way | nothing at head; the next test author. Fix named, not applied: keep `Ess` private to `suite/mod.rs` and expose only `synthesized` |
| F6 | `Taskfile.yml:54` | judgement | CONFIRMED | introduced | note | when the bound ends the step before cargo-nextest starts its run, `junit.xml` is the previous run's and the deferred list prints that run's ten slowest: probe with a 3 s bound that landed in compilation gave `status=124`, junit mtime unchanged, 1394 testcases of the earlier run | `task test-run` started on its own with tests not built (AGENTS.md lists the command alone), or a pool queue inside the bound. Not `task check`, not CI |

## 5. Attacked, not broken

- Stale suite: key holds relative names, length-prefixed bytes, sorted; no symlink under `ess/`; `ess verify conform synthesize` names no environment variable in its help, writes nothing into `ess/`, and gives byte-identical output for a relative and an absolute `--scenarios` path with no path in the suite, so two worktrees or a shared target directory cannot cross.
- Another `ess` binary with the same `--version`: not in the key; nothing found that reaches it (CI pins by sha256).
- `.part` left by a killed reader, lock of a killed process: `flock` ends with the process; `ess` overwrites an existing `--out` (measured, byte-identical result).
- A test changing what a later test reads: each reader gets its own `String`; `conform.rs` writes a copy into its run directory; nothing writes back.
- The "once" tests: `eight_processes_…` fails for a cache that always synthesizes (8 calls) and for one that never does.
- The bound inside a pool job, cut mid-tests: `timeout --kill-after=10s 3s cargo nextest run -p inference-testkit --all-features` gave `status=124 after 3 s`, a rewritten `junit.xml`, 0 test processes of that target left. The group SIGKILL left no orphan in a local probe.
- A part failing for another reason: exit 201, no bound line (local probe on a copy of the line).
- Queue wait: under `slot task test-run` the shim passes `cargo` through inside the job (`SLOT_JOB`), so the queue is outside the 120 s. Under plain `task test-run` on a pool host each of the three `cargo` calls queues and syncs inside the bound; read from the shim, not run (the agent hook refuses the plain form).
- The guide's listing equals `task --dry test-run-parts` in this tree.
- One hook refusal: `task test-run` on my scratch Taskfile (sleeps only) was refused by name; I renamed the scratch tasks and ran them outside the pool.

## 6. Paths written outside the worktree

Under `<scratch>/`:

- `bound/Taskfile.yml`
- `run1-own-cases.log`, `run2-build.log`, `run3-suite.log`, `run3-start`, `run3-end`, `run4-before.log`, `run5-bound.log`, `run6-bound.log`

On the build node, in the job's target directory: `tmp/adversary-w1-bound.log` was written by the two bound probes and removed by the second. The runs left three keys under `tmp/conformance-suite/` there, as any run of the crate does.

Lease `w1-adversary-1` released; worktree left in place.

```findings
- file: website/docs/guides/run-the-checks.md
  line: 41
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the guide and the commit message say task test-run exits 124 when its bound ends it, and task exits 201
- file: Taskfile.yml
  line: 35
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: when a part outlives SIGTERM by the kill-after time, timeout kills itself with its group, the status is 137 and neither the bound line nor the slowest list is printed; nothing found that outlives SIGTERM
- file: crates/inference-testkit/tests/suite/mod.rs
  line: 212
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a cold run of the testkit's tests calls synthesis three times, once per product, where the acceptance says once
- file: crates/inference-testkit/tests/suite/mod.rs
  line: 95
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: an empty suite.json is served as the suite and never synthesized again; only a machine crash between rename and write-back was found to produce one
- file: crates/inference-testkit/tests/conformance_suite.rs
  line: 255
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the scan matches one literal, so a test that synthesizes through the public suite::Cache and suite::Ess into a directory of its own passes every guard test
- file: Taskfile.yml
  line: 54
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: when the bound ends the step before cargo-nextest starts its run, the deferred list prints the ten slowest of the previous run from a stale junit.xml
```
