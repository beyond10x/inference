---
format: aep.planning-md/3
id: review-result:quality-records-written-by-each-check-run-adversary-2
kind: review-result
status: active
title: Adversary pass 2, after correction 1, over the records written by each check run (2026-11-26)
relations:
- reviews: story:quality-records-written-by-each-check-run
revision: 1
---
unit: story:quality-records-written-by-each-check-run, pass 2, worktree inference-quality-check-records at 2ffdc5c3e (split compared with 9afed267f)
verdict: CONFIRMED: 0 blockers, 0 warnings, 5 notes
cases: executed 122→148, red 4
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory (part 6)
needs-coordinator: none

**1. `git --no-pager diff --stat`** (tracked files, while the cases were in)
```
 crates/inference-docs/tests/quality/mod.rs     |   2 +
 crates/inference-docs/tests/quality/records.rs | 163 +++++++++++++++++++++++++
 crates/inference-docs/tests/workflows.rs       |  50 ++++++++
 3 files changed, 215 insertions(+)
```
- Untracked files that were in as well, all test files: `tests/adversary_shared/{plants.rs,split.rs}`, `tests/quality/{adversary2.rs,adversary_cmp.rs}`, `tests/quality_old/{mod.rs,adapters.rs,adversary_cmp.rs}`.
- `quality_old/` holds `mod.rs` and `adapters.rs` as they were at 9afed267f (`git show`), with only the `mod` lines changed. That is the documents test before the split, built in the same binary.
- No non-test path was touched. Everything is saved in `added-cases.patch` (10 file diffs) and was taken out with `git apply -R`. `git status --short --untracked-files=all` is empty, and the patch still applies to 2ffdc5c3e.
- I committed nothing, pushed nothing, wrote nothing under `.engineering/`, and checked out no other commit.

**2. Cases added** (each one was run alone, before the suite: `cargo nextest run -p inference-docs -E 'test(/adversary_(the_split|record_problems_refuses|the_records_task_asks|the_records_task_test_kills|the_continue_on_error_test)/)'`, exit 100, `6 tests run: 2 passed, 4 failed`)

- `adversary_split::adversary_the_split_finds_every_problem_it_found_before`: **GREEN**
  - It plants 12 defects and compares the *set* of `problems()` lines before and after the split.
  - The plants: none; wrong coverage count; wrong file name; wrong assessor; a listed test missing from a record; page values that disagree (a run value and a per-crate row); a test removed from a plan; a package removed from a plan; a record defect of one assessment plus a plan defect of a later one; a record that does not parse; a package result missing from a record; all of these at once.
  - The sets are equal for every plant.
- `adversary_split::adversary_the_split_finds_the_problems_in_the_same_order`: **RED**. Verbatim:
```
a record defect of one assessment and a plan defect of a later one: 7 problems before, 7 after; first difference at 1:
  before: Some("quality/records/aws-time-bounds.evidence.json: no result of `aws-time-bounds/held` for the test adapters/inference-aws/tests/nova_sonic_stall_and_bounds.rs::the_open_deadline_is_stated, which assessments.aws-time-bounds lists")
  after:  Some("assessments.deepgram-time-bounds: names the test `a_keep_alive_is_sent_after_silence_x` of `adapters/inference-deepgram/tests/audio.rs`, and that file has no `fn a_keep_alive_is_sent_after_silence_x`")
all of these together: 18 problems before, 18 after; first difference at 12: (same pair)
```
- `quality::adversary2::adversary_record_problems_refuses_a_record_without_the_output_of_a_measure`: **RED**
  - It removes the `slowest_test` result from `workspace-tests`, and the `inference-aws` `slowest_test` result from `adapter-tests`.
  - `problems()` names both ("has no value for the measure"; "no result of `adapter-tests/slowest-test` for inference-aws"). The assertion of that passed.
  - `record_problems()` finds nothing. Verbatim: `record_problems() holds both records without the output of a measure: 0 problem(s):` (adversary2.rs:51)
- `quality::records::adversary_the_records_task_asks_git_for_the_commit_of_head`: **GREEN** on the tree. This is the proposed fix.
  - A `git` stand-in logs its arguments. On the tree's commands the log is `rev-parse HEAD`.
  - On the mutant `git rev-parse HEAD^` it is not, and the case asserts that difference.
- `quality::records::adversary_the_records_task_test_kills_a_task_that_takes_another_commit`: **RED**
  - It runs the assertions of `the_records_task_writes_each_listed_record_and_fails_naming_every_refused_one`, unchanged, on the tree's commands: they pass.
  - It then runs them on the mutant `revision=$(git rev-parse HEAD^)`. Verbatim: `the mutant `revision=$(git rev-parse HEAD^)` passes every assertion of the_records_task_writes_each_listed_record_and_fails_naming_every_refused_one` (records.rs:555)
- `adversary_the_continue_on_error_test_kills_a_job_level_key_after_the_steps`: **RED**
  - It runs the assertions of `the_records_step_and_its_job_do_not_continue_on_error`, unchanged: they pass on `check.yml`.
  - The mutant appends `    continue-on-error: true` after the last step. `serde_norway` reads it as `/jobs/check/continue-on-error = true`, and the step count is unchanged.
  - The assertions pass on the mutant too. Verbatim: `a job-level continue-on-error after the steps passes the_records_step_and_its_job_do_not_continue_on_error` (workflows.rs:521)

Every case took under 1.5 s.

**3. Suite run, after the cases existed**
`cd <worktree> && cargo nextest run -p inference-docs`, exit 100:
```
    Starting 148 tests across 2 binaries
        FAIL [   0.014s] (  1/148) inference-docs::workflows adversary_the_continue_on_error_test_kills_a_job_level_key_after_the_steps
        FAIL [   0.140s] ( 28/148) inference-docs::workflows quality::adversary2::adversary_record_problems_refuses_a_record_without_the_output_of_a_measure
        FAIL [   1.432s] ( 64/148) inference-docs::workflows adversary_split::adversary_the_split_finds_the_problems_in_the_same_order
        FAIL [   0.262s] ( 93/148) inference-docs::workflows quality::records::adversary_the_records_task_test_kills_a_task_that_takes_another_commit
     Summary [   3.113s] 148 tests run: 144 passed, 4 failed, 0 skipped
error: test run failed
```
- The 122 before is the implementor's `cases:` line.
- 148 = 122 + 6 cases of mine + 20 tests of the pre-split copy (`quality_old::`). All 20 pass on the current tree.

**4. Findings** (tree 2ffdc5c3e)

1. **judgement, note, CONFIRMED, introduced.** The split keeps every problem but not their order.
   - What was measured: the order case above. `check()` now runs `check_named_records` after all of `check_named_tests` (`adapters.rs:650`). A record line of an earlier assessment now comes after the `check_test`/page lines of a later one.
   - What reaches it: two defects in different assessments at once.
   - Every consumer reads with `contains` or `is_empty`, so nothing depends on the order. Only the claim "in the same order" is wrong. Fix: correct the claim, or loop per assessment.
2. **acceptance, note, INFEASIBLE, introduced.** `record_problems()` (`mod.rs:1091`) does not require a record to hold the output of each measure of its plan.
   - `problems()` does require it, through the "entry names a run" rule (`check_entry`) and `check_crate_rows`.
   - What reaches it: nothing found. `assess` refuses when it cannot read a measure (`assess.rs:983-1000`), so it does not write such a record.
   - This is smaller than the acceptance word "outputs" suggests. Fix: add "one result per measure, and per package for a measure with `scope: package`" to `record_problems`.
3. **mutant, note, CONFIRMED, introduced.** The test of the task's commands does not hold which commit the records name.
   - What was measured: `STUB_GIT` (`records.rs:189`) answers the same commit whatever it is asked. A task with `git rev-parse HEAD^` (`Taskfile.yml:66`) passes every assertion of the test.
   - What reaches it: an edit to the task. Nothing found that does it today.
   - Fix: add `adversary_the_records_task_asks_git_for_the_commit_of_head` from the patch, or let the stand-in log its arguments and assert `rev-parse HEAD`.
4. **mutant, note, CONFIRMED, introduced.** The continue-on-error test reads only the lines of the job before `steps:` (`workflows.rs:298`).
   - A job-level `continue-on-error: true` written after the steps is a key of job `check` and passes the test.
   - What reaches it: an edit to the workflow. Nothing found that does it today.
   - Fix: parse `check.yml` with `serde_norway` and assert there is no `/jobs/check/continue-on-error`.
5. **contract-drift, note, CONFIRMED, introduced.** `quality/README.md:13-17` and `website/docs/guides/run-the-checks.md:174-179` list three ways the task fails. The code has more:
   - an assessor declaration that is missing or broken (`assess` exit 3, `assess.rs:375-383`);
   - a call error (exit 2).
   - `records.rs` itself names both as refused (`wrong-call`, `unreadable`). A change to `quality/assessors/` can therefore fail the gate, and the pages say only that a plan-file change can.
   - Found by reading; no case written. Fix: say "or when the program refuses or cannot write a record", or name the assessor declarations.

**5. Attacked, could not break**
- Split, set of problems: equal before and after for all 12 plants, including all of them at once.
- `record_problems` against the acceptance:
  - it holds format, project, file name, assessor, outputs declared, units, coverage counts, packages and coverage sums, and listed tests;
  - it checks no page value, and is a strict subset of `problems()`;
  - a broken plan file still produces record problems ("no assessment declared").
- Task mutants killed by the new test: deleting `exit 1`; deleting the `refused` append; overwriting `refused`; deleting the `written` increment; deleting the empty-list check; deleting `rm -rf`; deleting the plan-directory `mkdir`; swapping `--from`/`--source`.
- Task mutants with no effect in practice:
  - dropping `< /dev/null`: neither `assess` nor cargo reads stdin;
  - `&&` to `||` in the `GITHUB_*` condition: GitHub sets all three variables, and a local run sets none;
  - `= revision` to `!= document`: the tree has only these two kinds.
- `junit-assessments`: the profile filter, the printed-duration filter, the `command-run` exclusion, missing target kind, and missing `ratchet.toml` are all held by the new test. The plan order is not held, and is harmless.
- `check.yml`: step-level `continue-on-error` in any position, `run: task quality-records || true`, and a `run: |` block are all caught.
- Pages against the code: the README's list for `records.rs` and `junit-assessments`, and the new "documents are a gate" sentence, match the code apart from finding 5.

**6. Paths written outside the worktree**
- `<scratch>/evaluate/story2/adversary-2/` holds `added-cases.patch`, `cases.log` and `suite.log`.
- The test runs wrote `adversary-git-<pid>`, `adversary-existing-<pid>` and `records-task-<pid>`/`junit-assessments-<pid>` directories under `CARGO_TARGET_TMPDIR`. They are in the git-ignored `target/` of the build node's mirror of this worktree.

**7.**
```findings
- file: crates/inference-docs/tests/quality/adapters.rs
  line: 650
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "After the split problems() finds the same set of problems but not in the same order: a record line of an earlier assessment now follows the plan and page lines of a later one."
- file: crates/inference-docs/tests/quality/mod.rs
  line: 1091
  category: acceptance
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: "record_problems accepts a record that lacks the output of a measure of its plan, which problems() refuses, though assess never writes such a record."
- file: crates/inference-docs/tests/quality/records.rs
  line: 189
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The git stand-in answers one commit whatever it is asked, so a task that writes git rev-parse HEAD^ into every record passes the test of the task's commands."
- file: crates/inference-docs/tests/workflows.rs
  line: 298
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The continue-on-error test reads only the job lines before steps:, so a job-level continue-on-error: true written after the steps passes it."
- file: quality/README.md
  line: 13
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The README and run-the-checks.md list three ways task quality-records fails, but a missing or broken assessor declaration or a call error of assess fails it too."
```
