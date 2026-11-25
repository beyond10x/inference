---
format: aep.planning-md/3
id: review-result:quality-defined-in-ratchet-formats-adversary-1
kind: review-result
status: active
title: Adversary pass on the quality definition in Ratchet's formats
relations:
- reviews: story:quality-defined-in-ratchet-formats
revision: 1
---
Adversary, the one pass, on story:quality-defined-in-ratchet-formats, 2026-11-25, as the adversary returned it; `<scratch>` is the pass's scratch directory and `<quality scratch>` its parent, both outside the repository.

unit: story:quality-defined-in-ratchet-formats
verdict: red
cases: executed 40→51, red 11
origin: introduced 12, pre-existing 0, undecided 0
wrote-outside-worktree: `<scratch>/` only, 13 files (listed at the end)
needs-coordinator: yes

`<scratch>` is the adversary scratch directory of the brief, `<quality scratch>` its parent. Line numbers of `crates/inference-docs/tests/quality/mod.rs` are those of head `b77925eea`; my one wiring line moves everything after line 16 down by one in the worktree.

Needs the coordinator: finding 1 can only be fixed in `.github/workflows/check.yml` and `AGENTS.md:44`, which the implementor's brief put out of its reach. Finding 12 corrects one of the 16 findings routed to Ratchet.

## git --no-pager diff --stat (worktree, head still b77925eea, nothing committed)

```
 crates/inference-docs/tests/quality/mod.rs | 1 +
 1 file changed, 1 insertion(+)
```

`git status --short`:

```
 M crates/inference-docs/tests/quality/mod.rs
?? crates/inference-docs/tests/quality/adversary_q1.rs
```

The one line is `mod adversary_q1;` after `use crate::root;`. The new file has 247 lines and passes `rustfmt --edition 2024 --check`. No document, page or implementation file was edited; every planted defect is a change of an in-memory copy inside a case.

## Cases added, with the red output of the run alone

Order of runs: the cases were run alone before any suite run.

- `<scratch>/run-alone-1.txt`: 30 probes, 17 red, 13 green. The 13 green ones and 4 out-of-scope red ones were then removed from the file; the full probe file is kept as `<scratch>/adversary_q1-probes.rs`.
- `<scratch>/run-alone-2.txt`: 12 cases, 12 red. One mutant was changed here (a record naming an undeclared assessor instead of another declared one). One case (finding 6) was then removed as outside what the page claims.
- `<scratch>/run-alone-3.txt`: the final file, below.

Command: `cargo nextest run -p inference-docs --no-fail-fast adversary_q1`. Result: `Summary [   0.031s] 11 tests run: 0 passed, 11 failed, 40 skipped`, exit status 100.

Two cases on the tree as it is:

```
thread 'quality::adversary_q1::q1_a_pull_request_that_changes_only_the_quality_page_runs_the_check' panicked at crates/inference-docs/tests/quality/adversary_q1.rs:94:5:
assertion `left == right` failed: the scope step of check.yml skips the build and the tests for quality/README.md, which the quality check reads
  left: "planning"
 right: "full"

thread 'quality::adversary_q1::q1_a_plan_with_a_sampled_measurement_declares_its_conditions' panicked at crates/inference-docs/tests/quality/adversary_q1.rs:80:5:
these plans hold a sampled measurement and declare no observation conditions: ["sherpa-onnx-stt-resources", "whispercpp-stt-resources"]
```

Nine planted defects the check does not report (each panics with "the check reports no problem for this planted defect: ..."):

```
q1_planted_a_condition_unit_differs_from_the_declaration (adversary_q1.rs:116:5)
  the condition of test-budget is in ms, the assessor's output and the measurement in s
q1_planted_a_record_unit_differs_from_the_declaration (:132:5)
  the record of test-run-step states 101 ms, the assessor's output is in s
q1_planted_a_record_names_an_assessor_without_a_declaration (:148:5)
  the record of lint names command-run@2, which has no declaration
q1_planted_a_record_names_an_output_its_assessor_does_not_declare (:164:5)
  the record of lint holds a result of the output exit_code, which command-run@1 does not declare
q1_planted_an_assessment_names_an_undeclared_subject (:180:5)
  the assessment native-archive names the subject native-archives, which is not declared
q1_planted_a_gate_requires_an_undeclared_requirement (:196:5)
  the gate check requires formatted-renamed, which is no requirement
q1_planted_a_record_value_differs_from_the_baseline_of_the_page (:212:5)
  the record of workspace-tests holds 9.999 s, the page states the baseline 8.011 s
q1_planted_an_entry_of_the_page_loses_its_baseline (:227:5)
  the entry test-run-step/run-time of the page has no baseline and names no run
q1_planted_a_bound_of_the_specification_differs_from_the_page_and_the_taskfile (:243:5)
  test-step-budget is under 240 s, the page says under 120 s and Taskfile.yml ends the step at 120 s
```

## The suite

Command: `cargo nextest run -p inference-docs --no-fail-fast`. The 40 `PASS` lines and the per-test output blocks are cut here; the whole output is `<scratch>/run-suite-3.txt`.

```
slot: synced to build-1d in 0.7 s, 129570 bytes sent, at b77925eea + 2 changed files
   Compiling inference-docs v0.33.0 (.../crates/inference-docs)
    Finished `test` profile [unoptimized] target(s) in 0.27s
 Nextest run ID dd26b1f3-05d2-45b2-a90c-df27b3d688d2 with nextest profile: default
    Starting 51 tests across 2 binaries
     Summary [   0.377s] 51 tests run: 40 passed, 11 failed, 0 skipped
        FAIL [   0.007s] ( 4/51) inference-docs::workflows quality::adversary_q1::q1_a_pull_request_that_changes_only_the_quality_page_runs_the_check
        FAIL [   0.008s] ( 7/51) inference-docs::workflows quality::adversary_q1::q1_a_plan_with_a_sampled_measurement_declares_its_conditions
        FAIL [   0.019s] ( 9/51) inference-docs::workflows quality::adversary_q1::q1_planted_a_bound_of_the_specification_differs_from_the_page_and_the_taskfile
        FAIL [   0.019s] (10/51) inference-docs::workflows quality::adversary_q1::q1_planted_a_gate_requires_an_undeclared_requirement
        FAIL [   0.019s] (11/51) inference-docs::workflows quality::adversary_q1::q1_planted_a_condition_unit_differs_from_the_declaration
        FAIL [   0.019s] (12/51) inference-docs::workflows quality::adversary_q1::q1_planted_a_record_names_an_assessor_without_a_declaration
        FAIL [   0.018s] (13/51) inference-docs::workflows quality::adversary_q1::q1_planted_a_record_names_an_output_its_assessor_does_not_declare
        FAIL [   0.019s] (15/51) inference-docs::workflows quality::adversary_q1::q1_planted_a_record_unit_differs_from_the_declaration
        FAIL [   0.019s] (16/51) inference-docs::workflows quality::adversary_q1::q1_planted_a_record_value_differs_from_the_baseline_of_the_page
        FAIL [   0.019s] (20/51) inference-docs::workflows quality::adversary_q1::q1_planted_an_entry_of_the_page_loses_its_baseline
        FAIL [   0.025s] (23/51) inference-docs::workflows quality::adversary_q1::q1_planted_an_assessment_names_an_undeclared_subject
error: test run failed
exit status: 100
```

The 40 tests of the implementor's run all pass. No test over 5 s; the step ran 0.377 s.

## Findings

No wrong number was found: every baseline and every record value is in its named source (list under "Attacked and could not break").

1. `.github/workflows/check.yml:48` — warning, CONFIRMED, introduced. Reaches: the gate.
   - The arm `.engineering/* | *.md) ;;` gives a pull request that changes only `quality/README.md` the gate `planning`, so `task test-run` does not run.
   - The new check reads that page (`mod.rs:59`, `:236`). A page edit that drops a measure's heading passes the pull request and turns the push to `main` red.
   - Source: the red case above (`crate::gate("pull_request", &["quality/README.md"])` returns `planning`). The implementor's report names the same gap.
   - The fix lies in `check.yml:47` (a `quality/*` arm), a case for both sides in `crates/inference-docs/tests/workflows.rs`, and the wording at `AGENTS.md:44`.

2. `quality/adapters.ratchet.yaml:10` (plans at `:10-24` and `:26-40`) — warning, CONFIRMED, introduced. Reaches: a later Ratchet release reading the documents.
   - Both plans hold three `sampled` measurements and their `observation` (`:14-17`, `:30-33`) declares no `conditions`.
   - Source: `ratchet:docs/design.md:94` ("`ratchet validate` reports a sampled measurement that declares no conditions") and `ratchet:spec/domains/specification.yaml:479-485`.
   - Every other plan with a sampled measurement declares them (for example `quality/gate.ratchet.yaml:150-156`).

3. `crates/inference-docs/tests/quality/mod.rs:236` — warning, CONFIRMED, introduced. Reaches: the reader of the page.
   - The only tie between the page and the documents is that the text `` `<assessment>/<measurement>` `` occurs somewhere in the page.
   - Green for: a record value changed (8.011 to 9.999), a `Baseline:` line removed with its heading kept, a bound changed in the YAML (120 to 240), a record file removed.
   - Source: the story's planned scope, "or when the README's table and the documents disagree" (`<quality scratch>/story.md`); three red cases above.

4. `quality/README.md:30` (sentence at `:30-34`) — warning, CONFIRMED, introduced. Reaches: the reader of the page.
   - The page says the test fails "when a requirement, an assessment or a record names something that is not declared, when a unit differs from the assessor's".
   - Not held: a record's `assessor` and a result's `output` (`mod.rs:287-312` reads only `format`, `assessment`, `project`); the unit of a requirement's `condition` and of a record's result (`mod.rs:252-258` reads only the measurement's unit); an assessment's `subject`.
   - Five red cases above. Either the check gains these rules or the sentence is cut to what the check holds.

5. `crates/inference-docs/tests/quality/mod.rs:210` — warning, CONFIRMED, introduced. Reaches: the reader of the page.
   - `problems` has no rule for a gate's `requires`: a gate that lists a requirement that does not exist stays green.
   - A renamed requirement then leaves its gate without notice, while the page still says its bound must hold. Red case above.

6. `crates/inference-docs/tests/quality/mod.rs:228` — note, CONFIRMED, introduced. Reaches: nothing today; the reader of the page after a later change of `Taskfile.yml`.
   - Only a command that starts with `task ` is held against `Taskfile.yml`: 7 of the 16 commands. The other 9 are literal command lines.
   - Removing the `cargo clippy ... -D warnings` line from `check-source` leaves the check green (`<scratch>/run-alone-2.txt`), as does `command: env CI=1 task docs-chek` (`<scratch>/run-alone-1.txt`).
   - The page states this limit correctly, so no case stays in the file.

7. `quality/not-measured.md:40` — note, NEEDS-CHANGE, introduced. Reaches: the reader of the page.
   - "The licence of each library a native archive links" stands under "No command produces it" and says nothing is checked inside the archive.
   - `adapters/inference-sherpa-onnx/tests/licence.rs` (`no_gpl_phonemiser_is_linked`) scans the loaded sherpa-onnx and onnxruntime libraries for GPL phonemiser symbols in `task test-run` (check log: `PASS [   0.487s] (2354/3059)`).
   - The neighbouring section on usage says "Two tests hold parts of it"; this one needs the same sentence.

8. `quality/README.md:364` (also `:385`, `:421-422`; `quality/adapters.ratchet.yaml:54-60`) — note, NEEDS-CHANGE, introduced. Reaches: the reader of the page.
   - The page says the bound "under 1000" is asserted by the ignored test "when it is run", and the YAML puts it in a local gate `capability-record` only.
   - The gate also enforces it on the committed record in every `task test-run`: `the_resource_profile_is_a_measured_cpu_profile`, `(1..1_000).contains` at `adapters/inference-sherpa-onnx/tests/stt_record.rs:106` and `adapters/inference-whispercpp/tests/stt_record.rs:67`.

9. `quality/README.md:218` — warning, NEEDS-CHANGE, introduced. Reaches: the reader of the page.
   - Bounds that are enforced today stand in neither page: Docs UX at 60 s per test and 120 s per run (`ux/playwright.config.ts:6-7`), its step at 2 minutes (`.github/workflows/docs-ux.yml:44`), the job timeouts of `docs-ux` (20 min, `:23`) and `slow` (30 min, `.github/workflows/slow.yml:47`).
   - Their values exist too: `ux: 45 tests, wall 48.0 s` in the docs run log, `task test-slow` 76 s in `<quality scratch>/ci/run-45474240955.json`.
   - The page gives the same pair (budget per test, budget per step, job timeout) for the Rust side only.

10. `quality/records/docs-ux.evidence.json:11` (and `quality/README.md:48`) — note, NEEDS-CHANGE, introduced. Reaches: a later Ratchet release reading the documents.
    - The record says `selection: candidate` at revision `adf479ee`. The run checked out `refs/remotes/pull/66/merge` (`HEAD is now at 1ed5587 Merge adf479ee... into 21cab787...`), which the format calls `merge_result`.
    - The values hold: the merge commit, `adf479ee` and `59b8f4a8` have the same tree `17ea6f2e`.

11. `quality/gate.ratchet.yaml:147` — note, NEEDS-CHANGE, introduced. Reaches: the reader of the page.
    - The plan `slow-lane` runs in `[local, ci]` with `command: task test-slow`. Locally the task has `sources:` (`Taskfile.yml:59-68`) and produces nothing when none changed; the page (`quality/README.md:169`) does not say so.
    - The gate's `when.changed` list (`quality/gate.ratchet.yaml:318-329`, 11 paths, as `slow.yml`) is not the task's local list (9 paths: without `Taskfile.yml` and `.github/workflows/slow.yml`).

12. `<quality scratch>/implementor-report.md:120` — note, NEEDS-CHANGE, introduced. Reaches: nothing in the repository; the list sent to Ratchet.
    - Finding F9 for Ratchet says the meaning of `holds` on a scope other than `run` is not stated.
    - It is stated: `ratchet:docs/design.md:181`, "`holds` requires it for every item, and the result names the items where it does not hold".

## Planted defects

"red" means the check reports a problem, "green" means it stays green. All planted in memory; evidence in `<scratch>/run-alone-1.txt` unless a later run is named.

| Planted defect | Check |
| --- | --- |
| requirement names an undeclared measurement (`checksum-matches` to `checksum`) | red |
| assessment names an assessor without a declaration (`generated-files@2`) | red |
| measurement unit differs from the declaration (`scenarios` to `files`) | red |
| condition unit differs (`test-budget`, `s` to `ms`) | green |
| record result unit differs (`test-run-step`, `s` to `ms`) | green |
| `task <name>` that does not exist inside a longer command line (`env CI=1 task docs-chek`) | green |
| record with another `format` | red |
| record with another `project` | red |
| record of an undeclared assessment | red |
| record naming another declared assessor (`nextest-run@1`) | green |
| record naming an undeclared assessor (`command-run@2`, run-alone-2) | green |
| record result of an undeclared output | green |
| record value differs from the page's baseline | green |
| record file removed while the page gives its run | green |
| measure removed from the page (heading removed) | red |
| `Baseline:` line removed, heading kept | green |
| requirement removed from every gate (legal in the format: observe mode) | green |
| gate lists an undeclared requirement | green |
| assessment names an undeclared subject | green |
| YAML bound changed (120 to 240) | green |
| `cargo clippy` line removed from `Taskfile.yml` | green |
| operator the specification does not declare (`under`) | green |
| key the specification does not declare (`severity`) | green |
| `quality/extra.ratchet.yaml` not in `[spec] paths` | red |

- Not planted, by reading `mod.rs:39-53`: a file under `quality/` with another suffix or in a subdirectory is not read at all, so nothing reports it.
- The rules of the implementor's brief (item 7): every one I planted is held. Three I did not plant and read only: a file that does not parse, `format` or `project` differing between specification files, a requirement naming an undeclared assessment.

## Attacked and could not break

- Step times 224, 233, 101 s and job time 589 s: equal to `<quality scratch>/ci/run-45474241015.json`.
- Queue wait 284 s: run creation 19:49:16 to job start 19:54:00. The run file holds no job creation time; the page says "from the creation of the run", which is what the file gives.
- 3059 run, 3059 passed, 72 skipped, slowest 8.011 s, stack test 23, doc tests 38: in the check log.
- 11 files, 10 scenarios, 6 generated files current, 472 conformance ids (17 arrays in `crates/inference-testkit/tests/conform.rs`, all unique): hold.
- Slow lane 26.703 s, 1 test: in the slow log.
- Docs UX: `45 passed`, `ratchet: 80 values, 0 problems`; the 80 record results equal `ux/baseline.json` and the page's tables.
- Capability numbers 131 / 4937 / 363 and 202 / 630 / 337: equal to the two `stt-i9-10900k-cpu.json`.
- Page against records: every run-scope value stands as `Baseline: <value>` in its entry (probe, green, 15 or more compared).
- Bounds: `lt 60` (`.config/nextest.toml`, kill at 60 s), `lt 120` (`Taskfile.yml:35`), `lt 2700` (`check.yml:23`), `lt 600` (slow profile, 60 s times 10), `lt 1000` (the ignored tests assert `< 1_000`): each enforced with that operator. No bound added, tightened or loosened.
- Commands: every `task` exists; the cargo lines stand in `check-source`; the conformance test runs inside `cargo nextest run --workspace --all-features`; both ignored tests exist under the stated name and assert the bound.
- Keys and closed values of the three `ratchet/1` files, the eight declarations and the 15 records: all declared at the pin (three probes, green).
- Joins: subjects, assessors, target kinds, inputs, scopes, boundaries, identities, operator against value type, condition units, gate entries, gate classes, every requirement in a gate (probe, green).
- Records: rules 1, 2, 3, 4 and 9, `samples` and `run.started_at`/`ended_at` on sampled results, assessor, target, inputs, conditions and coverage unit against plan and declaration, a result for every measurement (probe, green).
- `runs: 1` with a sampled measurement: valid (`ratchet:spec/domains/specification.yaml:185`).
- `quality/not-measured.md`: no bench target, no coverage, audit or semver tooling, `unsafe_code = "forbid"` in all crates, the two usage tests exist, no retries in nextest; no number without a file.
- `quality/README.md`: says no Ratchet command works and the names are proposals; every entry has command, unit, direction, bound or "no bound", baseline and source.
- Public repository: no person's name, login, home path, private repository or session name in the added files or the commit message; `beyond10x/ratchet` and `beyond10x/gates` are public.

## Written outside the worktree

All under `<scratch>/`: `check.txt`, `slow.txt`, `docsux.txt` (the CI logs without colour codes), `baseline.tsv`, `record.tsv`, `consts.txt`, `adversary_q1-probes.rs`, `run-alone-1.txt`, `run-alone-2.txt`, `run-alone-3.txt`, `run-suite.txt`, `run-suite-2.txt`, `run-suite-3.txt`.

- The build pool synced the tree to its build host for each `cargo nextest` run; no `target/` exists in the worktree.
- The lease `quality-adversary-1` was started and ended with the `worktree` hooks.
- No permission check refused anything. Nothing committed, pushed, stashed or switched.

```findings
- file: .github/workflows/check.yml
  line: 48
  category: gate
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: A pull request that changes only quality/README.md gets the gate "planning", so the quality check, which reads that page, does not run on it and first fails on main.
- file: quality/adapters.ratchet.yaml
  line: 10
  category: specification
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: The plans sherpa-onnx-stt-resources and whispercpp-stt-resources hold sampled measurements and declare no observation conditions, which the pinned specification has ratchet validate report (docs/design.md:94, spec/domains/specification.yaml:479-485).
- file: crates/inference-docs/tests/quality/mod.rs
  line: 236
  category: check
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: The check ties the page to the documents only by the measure's name; a changed record value, a removed baseline line, a changed bound and a removed record stay green, against the story's "when the README's table and the documents disagree".
- file: quality/README.md
  line: 30
  category: page
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: The page says the check fails when a record or an assessment names something undeclared and when a unit differs from the assessor's; a record's assessor and output, a condition's unit, a record's unit and an assessment's subject are not held.
- file: crates/inference-docs/tests/quality/mod.rs
  line: 210
  category: check
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: No rule reads a gate's requires list; a gate that names a requirement that does not exist stays green, so a renamed requirement leaves its gate unnoticed.
- file: crates/inference-docs/tests/quality/mod.rs
  line: 228
  category: check
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Only the 7 of 16 commands that start with "task " are held against Taskfile.yml; removing the cargo clippy line from check-source, or a task name inside a longer command line, stays green.
- file: quality/not-measured.md
  line: 40
  category: page
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The licence of the linked native libraries is listed as produced by no command, while adapters/inference-sherpa-onnx/tests/licence.rs scans the loaded libraries for GPL phonemiser symbols in task test-run.
- file: quality/README.md
  line: 364
  category: page
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The real-time factor bound is said to be asserted only by the ignored test when it is run; the gate also enforces it on the committed record in stt_record.rs of both adapters on every task test-run.
- file: quality/README.md
  line: 218
  category: page
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: Bounds enforced today stand in neither page - Docs UX 60 s per test, 120 s per run and a 2 minute step, the job timeouts of docs-ux (20 min) and slow (30 min) - with their values (48.0 s, 76 s) in the CI files.
- file: quality/records/docs-ux.evidence.json
  line: 11
  category: record
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The record says selection candidate at adf479ee; the run checked out the merge result 1ed5587 (refs/remotes/pull/66/merge), which the format calls merge_result. The trees are equal, so the values hold.
- file: quality/gate.ratchet.yaml
  line: 147
  category: specification
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The slow-lane plan runs in local with task test-slow, which locally produces nothing when none of its 9 sources changed; the gate's when.changed list has the workflow's 11 paths and the page does not name the difference.
- file: implementor-report.md
  line: 120
  category: routing
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: Finding F9 for Ratchet (the meaning of holds on a scope other than run is not stated) is contradicted by the pinned docs/design.md:181, "holds requires it for every item".
```
