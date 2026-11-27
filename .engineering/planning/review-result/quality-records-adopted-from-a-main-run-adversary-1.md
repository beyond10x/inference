---
format: aep.planning-md/3
id: review-result:quality-records-adopted-from-a-main-run-adversary-1
kind: review-result
status: active
title: Adversary pass over the adopt command (2026-11-26)
relations:
- reviews: story:quality-records-adopted-from-a-main-run
revision: 1
---
unit: story:quality-records-adopted-from-a-main-run phase 1 (the adopt-records command), wave2/adopt-records at 6acbb8333 (base ae84c5f92)
verdict: findings: 4 (blocker 0 / warning 0 / note 4); 3 CONFIRMED red cases are INFEASIBLE by reach, 1 mutant CONFIRMED
cases: executed 148→155, red 3
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 6 paths (part 6)
needs-coordinator: the brief asks me to plant defects in src (`record_rules`, `Plan::of`). My charter forbids changing a file under attack, even briefly, and the brief forbids building a copy. So I planted no src mutants (part 5).

No finding holds the unit. Every acceptance refusal I tried is refused. The 4 findings are notes: 3 inputs nobody is shown to reach, and 1 drift test that checks one sample per rule.

## 1. Diff stat while the cases stood
```
 crates/inference-docs/tests/quality/adopt.rs | 2 ++     (`mod adversary;` at the end)
 + untracked crates/inference-docs/tests/quality/adopt/adversary.rs (new, 7 tests)
```
Both are test paths. Plants M1 and M2 were in test files only and were restored before the next run. The cases are removed again; `git status --short` is empty. Saved as `added-cases.patch`, which applies to 6acbb8333.

## 2. Added cases (file `tests/quality/adopt/adversary.rs`), first run alone
Command: `cargo nextest run -p inference-docs -E 'test(/adopt::adversary::/)'` → `7 tests run: 4 passed, 3 failed`, exit 100.

- `a_record_that_holds_a_key_twice_is_refused`: **red**. Puts `"project": "elsewhere"` before `"project": "inference"` in aws-records.
  - Output: `panicked at …/adversary.rs:21:10: assertion left == right failed: stderr: (empty) left: Some(0) right: Some(1)`
- `a_link_in_the_place_of_a_temporary_file_is_not_written_through`: **red**. A symlink `.aws-records.evidence.json.adopting` points to `<case>/outside.txt`.
  - Output: `panicked at …/adversary.rs:38:5: the file the link names was written; status Some(0), stderr: (empty)`.
  - `left` is the bytes of the aws-records record (`{\n  "format": "ratchet-evidence/1",…`); `right` is `outside\n`. The program writes the record through the link, then renames the link onto `quality/records/aws-records.evidence.json`.
- `a_target_that_assess_never_writes_is_refused`: **red**. Two plants: a revision target that also has `identifiers` (path `Cargo.toml`), and a document target with `digest_form: "text"`.
  - Output: `panicked at …/adversary.rs:74:10: left: Some(0) right: Some(1)`, stderr empty.
- `a_second_plant_of_each_rule_is_named_by_both_in_the_same_words`: green. 7 plants the drift test does not have: unit removed, `not_run`=2, format removed, `passed`+1, `unsupported` removed, project removed, output removed. It needs `record_problems()` and the program to name each in the same words.
- `the_inputs_of_the_brief_are_refused`: green, 0.99 s. It covers:
  - `headBranch` set to `refs/heads/main` or `main `;
  - `pull_request` on main;
  - no `workflowName` or `databaseId`;
  - a symlink record;
  - `AWS-records` (other case);
  - a second record under another plan directory;
  - a trailing `{}` document;
  - another repository with the same run id;
  - `http://`;
  - a `--root` whose plan lists another test.
- `a_records_directory_that_is_read_only_leaves_every_record`: green. It returns early if the directory stays writable, as it would for root on the build node. Whether it did that early return was not recorded.
- `a_second_run_after_a_kill_between_two_renames_adopts_every_record`: green. I planted half the records adopted and `.adopting` files for the rest; a second run adopted all and left no temporary file.

Mutant probes (test files only, each restored before the next run):
- M1: drop `"not_run"` from `COVERAGE_COUNTS` (`tests/quality/mod.rs:42-49`).
- M2: drop `("passed","tests_passed")` (`tests/quality/adapters.rs:593`).
- Result: under each mutant the drift test `a_record_the_record_rules_refuse_is_refused_by_the_program_in_the_same_words` stayed **PASS**, and my variant case went **FAIL**:
  - M1: `record_problems() does not name "the coverage has Some(2) for \`not_run\`…" for quality/records/cartesia-records.evidence.json`
  - M2: `… does not name "passed tests, and the \`tests_passed\` results …" for quality/records/adapter-tests.evidence.json`

## 3. Suite run (after the cases existed)
Command: `cd <worktree> && cargo nextest run -p inference-docs`
```
     Summary [   4.314s] 155 tests run: 152 passed, 3 failed, 0 skipped
        FAIL quality::adopt::adversary::a_record_that_holds_a_key_twice_is_refused
        FAIL quality::adopt::adversary::a_target_that_assess_never_writes_is_refused
        FAIL quality::adopt::adversary::a_link_in_the_place_of_a_temporary_file_is_not_written_through
exit 100
```
- `before` (148) is the implementor's green run.
- No test is over 5 s.

## 4. Findings (tree 6acbb8333)
1. **Duplicate keys are not refused.** `src/assess/adopt.rs:177`
   - `serde_json::from_slice` keeps the last value of a repeated key, and the test side does the same. The bytes are copied as they are, so a reader that keeps the first value reads another project.
   - What reaches it: nothing found. `assess` writes through serde and never repeats a key; only a hand-edited or tampered artifact has one. INFEASIBLE, note.
   - Fix: refuse a key that appears twice when parsing, for example by deserialising through a map type that errors on duplicates.
2. **The temporary file is written through a symlink.** `src/assess/adopt.rs:631` (`fs::write(&one.temporary…)`), then rename at `:641`
   - A pre-existing link at the temporary path gets the record bytes written outside the tree, and the record ends up as a link.
   - What reaches it: nothing creates such a link. INFEASIBLE, note.
   - Fix: `remove_file` the temporary path first (this removes the link, not its target), then open it with `OpenOptions::new().write(true).create_new(true)`.
3. **The target's shape is not checked against what `assess` writes.** `src/assess/adopt.rs:366-406`
   - A revision target may carry `identifiers` (a path and digest of another file), and a document target's `digest_form` is unchecked.
   - What reaches it: `assess` never writes either (`src/assess.rs:1547-1563`). INFEASIBLE, note.
   - Fix: refuse `identifiers` on a revision target, and require `digest_form == "bytes"`.
4. **The drift test checks one input per rule.** `tests/quality/adopt.rs:525` (`PLANTS`) and `:668`
   - It catches a rule removed on one side, not a rule narrowed on one side. M1 and M2 survive it.
   - The src side was not mutated (charter). My variant case would catch the same narrowings there, by construction.
   - What reaches it: a later edit of either copy. The damage is bounded: committed records still go through `problems()` in `the_quality_documents_agree_with_each_other_and_with_the_taskfile`, which runs the test-side rules. CONFIRMED, note.
   - Fix: add the 7 `VARIANTS` of the patch to `PLANTS`, or drive both sides from one table of plants.

## 5. Attacked, not broken
- Every input in the brief's list is refused; case `the_inputs_of_the_brief_are_refused`.
- run.json with extra fields is accepted (not a defect).
- A read-only records directory and a write failure leave all records as they were. A temporary name that collides with a stale regular file is overwritten and consumed.
- A kill between two renames leaves a mix: k new records, 22−k old ones and their `.adopting` files. That mix cannot be planted from outside, but a rerun repairs it (case above). README promises all-or-none only for a write that fails, not for a kill.
- `Plan::read` refactor:
  - read against `git show`: the "first plan file wins" behaviour is unchanged and the project check stays per file;
  - base run 140/140 (the implementor's `base-run.log`, at ae84c5f92 with 0 changes), head 148/148;
  - I did not run the base myself (no base tree; I may not move this one);
  - the only behaviour a `Plan::of` mutant could change (last instead of first) needs the same assessment id in two plan files, which `tests/quality/mod.rs:216` refuses. INFEASIBLE, not planted.
- Docs: the README section "Adopting the records of a run on `main`" and the `AGENTS.md` line agree with the code. One small gap I left as no finding: exit 3 also covers a missing target document of the tree, and neither the README nor the module doc says so.

## 6. Paths written outside the worktree
All under `<scratch>/evaluate/wave2-adopt-adversary/`:
- added-cases.patch
- cases-alone.log
- mutant-m1.log
- mutant-m2.log
- suite.log
- diff-stat.txt

Test scratch from the runs went under the worktree's `target/` (`CARGO_TARGET_TMPDIR`). Lease `wave2-adopt-adversary` was acquired and released.

```findings
- file: crates/inference-docs/src/assess/adopt.rs
  line: 177
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: "A record holding a key twice is adopted, because serde_json keeps the last value and the copied bytes then say two things to readers that keep the first."
- file: crates/inference-docs/src/assess/adopt.rs
  line: 631
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: "A symlink at a temporary path is written through to a file outside the tree and then renamed into place as the record."
- file: crates/inference-docs/src/assess/adopt.rs
  line: 366
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: "A target that assess never writes is adopted: a revision target with identifiers naming another file, or a document target with a digest_form other than bytes."
- file: crates/inference-docs/tests/quality/adopt.rs
  line: 525
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The drift test plants one input per rule, so a rule narrowed on one side survives it, as shown by test-side mutants dropping not_run and the tests_passed sum."
```
