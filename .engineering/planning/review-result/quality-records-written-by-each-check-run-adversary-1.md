---
format: aep.planning-md/3
id: review-result:quality-records-written-by-each-check-run-adversary-1
kind: review-result
status: active
title: Adversary pass 1 over the records written by each check run (2026-11-26)
relations:
- reviews: story:quality-records-written-by-each-check-run
revision: 1
---
unit: story:quality-records-written-by-each-check-run, worktree inference-quality-check-records at 1c0f41984 (base 6a59105d9)
verdict: NEEDS-CHANGE: 1 blocker, 1 warning, 3 notes
cases: executed 118→121, red 1
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory (part 6)
needs-coordinator: finding 1 needs the owner to choose between amending the acceptance clause and changing the check. The record check (`mod.rs:707`) and the pages are out of scope under the story's own "Out of scope".

**1. `git --no-pager diff --stat`** (worktree, while the cases were in)
```
 crates/inference-docs/tests/quality/assess.rs  |  52 +++++++
 crates/inference-docs/tests/quality/records.rs | 192 +++++++++++++++++++++++++
 2 files changed, 244 insertions(+)
```
Both are test files. The cases are saved as `added-cases.patch` and taken out with `git apply -R`. `git status --short` is empty now. I committed nothing, pushed nothing and wrote nothing under `.engineering/`.

**2. Cases added** (each one was run alone before the suite ran)

- `quality::assess::adversary_a_record_as_the_records_task_writes_it_is_accepted_in_the_place_of_its_record` (RED)
  - It writes `workspace-tests` the way the task writes it: `--class ci --source https://github.com/beyond10x/inference/actions/runs/1`, then `--class local --source target/nextest/default/junit.xml`, with `--revision` of another commit.
  - The report is the fixture whose values the pages state, so only the run and the commit differ from the committed record.
  - It puts that record in the place of the committed one and asserts `problems()` is empty.
  - Red output, verbatim (`cargo nextest run -p inference-docs -E 'test(adversary_a_record_as_the_records_task_writes_it)'`, exit 100):
```
thread 'quality::assess::adversary_a_record_as_the_records_task_writes_it_is_accepted_in_the_place_of_its_record' (3971086) panicked at crates/inference-docs/tests/quality/assess.rs:1967:9:
class ci: 1 problem(s):
quality/README.md: the entry `workspace-tests/slowest-test` names the run 45474241015, the record is of "https://github.com/beyond10x/inference/actions/runs/1"
     Summary [   0.105s] 1 test run: 0 passed, 1 failed, 118 skipped
```

- `quality::records::adversary_the_records_task_writes_each_listed_record_and_fails_naming_every_refused_one` (GREEN on the tree; it rewrites a test that cannot fail)
  - It takes the `cmds` of `quality-records` from `Taskfile.yml` and runs them with `sh` in order, stopping at the first that fails. A stub `cargo` and `git` stand in on PATH.
  - A refused assessment is named and fails the task for exit 1, 2 and 3.
  - Written records are written, and a stale record from an earlier run is removed.
  - `--revision` goes only to targets of kind `revision`.
  - Class and source are `ci` with the forge URL when the three `GITHUB_*` variables are set, `local` with the report path when they are not.
  - An empty list fails with "listed no assessment".

- `quality::records::adversary_junit_assessments_leaves_out_another_profile_and_the_printed_durations` (GREEN on the tree; it kills surviving mutants)
  - On a scratch root, under `task test-run`, these are not listed: profile `slow`, profile `stack`, `printed-duration@1`, `command-run@1`.
  - `nextest-run@1` with no profile is listed, and so is `named-tests@1` with a `document` target.
  - A listed assessment without a target kind exits 3, and so does a root without `ratchet.toml`.

Both green cases take under 0.1 s.

Mutant probe for case 2: `adversary-1/probe/run.sh` runs the task's loop script and its mutants with `sh` and stubs, never on the tree. Output, verbatim:
```
original: exit 1
task quality-records: no record of refused-one
      3 --class local
mutant-no-exit: exit 0
task quality-records: no record of refused-one
      3 --class local
mutant-class: exit 1
      3 --class ci
mutant-revision: exit 1
doc-one calls with --revision: 1
```
- Every mutant still holds each substring that `records.rs:155` checks, so the existing suite stays green on all three.
- Case 2 asserts `status != 0`, `--class local` and no `--revision` for `doc-one`, so it goes red on each mutant.

**3. Suite run, after the cases existed**
`cd <worktree> && cargo nextest run -p inference-docs`, exit 100:
```
    Starting 121 tests across 2 binaries
        FAIL [   0.130s] ( 60/121) inference-docs::workflows quality::assess::adversary_a_record_as_the_records_task_writes_it_is_accepted_in_the_place_of_its_record
     Summary [   1.401s] 121 tests run: 120 passed, 1 failed, 0 skipped
error: test run failed
```
The 118 before is the implementor's `cases:` line.

**4. Findings** (tree 1c0f41984)

1. **acceptance, blocker, NEEDS-CHANGE, introduced.** No record the task writes can satisfy "accepted by the test of the quality documents in the place of that assessment's record".
   - What was measured: case 1 above. The problem comes from `crates/inference-docs/tests/quality/mod.rs:707`: every page entry names run 45474241015, and a record from any other run, CI or local, is refused.
   - The implementor saw 86 such problems with real local records.
   - What reaches it: every run of `task quality-records`.
   - The story's "Out of scope" forbids changing pages and records. The owner either amends the acceptance clause (for example "accepted by the record checks, `check_records`") or allows a change to the check.

2. **mutant, warning, CONFIRMED, introduced.** None of the task's behaviour is held by a program.
   - What was measured: `records.rs:132`/`:155` checks only that 4 substrings are present.
   - What reaches it: deleting the `exit 1` of the refused branch (`Taskfile.yml:92`) gives exit 0 with a refused record. Swapping `ci`/`local` passes too.
   - Fix: add case 2 (from the patch) to the suite.

3. **mutant, note, CONFIRMED, introduced.** The `profile` filter (`assess.rs:460`) and the `PrintedDuration` filter (`assess.rs:449`) of `junit-assessments` never act on the tree.
   - No plan file has a `task test-run` assessment with a non-default profile or of `printed-duration@1`.
   - Dropping either filter changes nothing that `records.rs` reads.
   - Fix: add case 3.

4. **mutant, note, CONFIRMED, introduced.** The workflow tests check `if:` but not `continue-on-error`.
   - Adding `continue-on-error: true` to the step `task quality-records` (`check.yml:147`) leaves `workflows.rs` green.
   - The forge would then pass a run whose records were refused, against "the local gate refuses what the forge would refuse". Found by reading the tests; I wrote no case.
   - Fix: assert that the step and the job carry no `continue-on-error`.

5. **contract-drift, note, CONFIRMED, introduced.** The pages say something the code no longer does.
   - `quality/README.md:13` and `website/docs/guides/run-the-checks.md:174` now say "The documents change no gate", then say a step of `task check` reads them and fails.
   - A plan-file edit can now fail `task check` and the required CI check, so the documents do add a gate condition.
   - Fix: say that.

**5. Attacked, could not break**
- Shell: glob characters in names (all quoted); `set --` in the loop; stdin (`< /dev/null`); exit codes 2 and 3 are named as refused.
- A failing list command stops the task, being its own `cmds` entry. Without errexit the final status is still right; an empty list fails.
- A stale `target/quality-records/` is removed. A stale `junit.xml` cannot occur inside `task check` or CI, because `test-run` failing stops both.
- A name with a space: the program does not validate it, and the task splits such a line wrongly but still fails loudly. I could not show any plan reaching this.
- Workflow: the triggers are `pull_request` and `push: branches: [main]` only (no `workflow_dispatch`, `merge_group` or tag push), so the conditions the tests evaluate match the reachable events.
- `||` in a condition gets past the evaluator's claim that other forms fail, but the push-to-main test goes red on any such mutant.
- The pin `cf430e030ddbb5b0abf93d22962f4752f3646cd9` is `refs/tags/v7.0.2` (`git ls-remote`).
- The upload is skipped after a failed task, so `if-no-files-found: error` can never fire. Harmless, and part of the design.
- Build freshness: by reading, `docs-check` builds the dev binary and later steps do not invalidate its fingerprint. Not measured: the brief allows me no build command other than the two named.
- Pages against the code: the README's commands, the paths, `gh run download` and the AGENTS.md lines all match the code.

**6. Paths written outside the worktree**
- `<scratch>/evaluate/story2/adversary-1/`, holding:
  - `added-cases.patch`, `case-a.log`, `case-bc.log`, `suite.log`
  - `probe/`: `script.sh`, `run.sh`, `original.sh`, `mutant-*.sh`, `bin/cargo`, `bin/git`, `work-*/`, `mutants.log`
- The test runs also wrote `adversary-records-task-<pid>`, `adversary-list-<pid>` and `assess-<pid>-*` under `CARGO_TARGET_TMPDIR`, in the git-ignored `target/` of the build node's mirror of this worktree.

**7.**
```findings
- file: crates/inference-docs/tests/quality/mod.rs
  line: 707
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "A record task quality-records writes, from CI or locally, is refused in the place of its committed record because every page entry names run 45474241015, so the story's acceptance clause cannot hold for any run."
- file: crates/inference-docs/tests/quality/records.rs
  line: 155
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: "The only test of the task checks substrings, so deleting the refused branch's exit 1 or swapping the ci and local class keeps the suite green."
- file: crates/inference-docs/src/assess.rs
  line: 460
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The profile and printed-duration filters of junit-assessments act on no assessment of the tree and no fixture test holds them."
- file: .github/workflows/check.yml
  line: 147
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The workflow tests do not read continue-on-error, so a step task quality-records that the forge lets fail passes them."
- file: quality/README.md
  line: 13
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The README and run-the-checks.md say the documents change no gate while a step of task check now fails on them."
```
