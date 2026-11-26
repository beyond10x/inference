---
format: aep.planning-md/3
id: story:quality-records-written-by-each-check-run
kind: story
status: implemented
title: Each check run writes the quality records its JUnit report holds and keeps those of main as an artifact
relations:
- serves: vision:inference
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T03:26:52Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T03:26:52Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-26T06:32:39Z", actor: "human:operator", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":2,"verification":1}}}
---
## Context

`inference-docs assess` writes the record of one assessment from what a run left behind
(`story:quality-records-written-by-a-program`). Nothing calls it: the records under
`quality/records/` are still the ones written by hand from four CI runs of 2026-11-24, and a
`check` run on `main` leaves no record behind. The owner wrote on 2026-11-25: "we also want to
evaluate it, right, and track over time."

The workflow `check` runs the full gate on every push to `main`
(`.github/workflows/check.yml`). After its step `task test-run` the JUnit report of the workspace
run exists on the runner. Of the measures of the three plan files, the ones the assessors
`nextest-run`, `package-tests`, `named-tests` and `capability-record` read come from that report.

This story makes each `check` run write those records and hands the ones of a run on `main` over
as a forge artifact. It keeps no series: where the series lives is the owner's open decision
(D48 of 2026-11-25), and an artifact is the first step of each of its options.

## Acceptance

Given a tree whose `task test-run` ended green, when `task quality-records` runs, then one
`ratchet-evidence/1` document per assessment whose record the JUnit report of that run holds
exists under `target/quality-records/`, each with the format, project, file name, assessor,
outputs, units, coverage and packages that the test of the quality documents demands of that
assessment's record, and the task exits non-zero and names every assessment whose record was
refused; and when the workflow `check` ran for a push to `main`, then its run has one artifact
that holds those documents and names the commit.

The values the pages of `quality/` name (the baselines and the run they were read from) are not
part of this acceptance: they name the check run of 2026-11-24, so a record of any later run
differs from them until the story after this one adopts a run on `main`. Decided by the
coordinator on 2026-11-26, after the implementor's records check of a real local run found 86
differences, all of them page values and none in a record.

## Planned scope

- A task `quality-records` in `Taskfile.yml`: one call of the program per assessment that reads
  the JUnit report, the list taken from the plan files and the declarations, never written a
  second time in the task. `task check` runs it after `test-run`, so that the local gate refuses
  what the forge would refuse. It builds nothing that `test-run` did not build and stays outside
  the test step's 120 s.
- `.github/workflows/check.yml`: the step `task quality-records` after `task test-run` in the
  full gate, and, for a push to `main` only, the upload of `target/quality-records/` as one
  artifact named with the commit. The action is pinned by its commit id like the checkout.
- `crates/inference-docs/tests/workflows.rs`: the cases for both sides (a push to `main` uploads,
  a pull request does not; the planning gate runs neither step).
- `quality/README.md`: where the records of a run are and how to fetch them.
- `AGENTS.md`: the task in the list of what `task check` runs, if it lists them.

## Out of scope

- The series: a branch, a ref, a download into a kept directory, a `quality-history` command.
- A change of a committed record, a plan file or a page entry: the adoption from a run on `main`
  is the story after this one.
- The measures read from the forge's job and step times and from a scan's conclusion: they need
  a reader and a `workflow_run` job with `actions: read`, a story of their own.
- The workflows `slow` and `Docs UX`, a schedule, a secret, a keyed run.
- The cancellation setting of `check` on `main` (a cancelled run leaves no artifact).

## Scope as built

Built on the branch `quality/check-records`, merged into `wave/2026-11-26-quality`:

- `inference-docs junit-assessments [--root .]` (`crates/inference-docs/src/assess.rs`, wired in
  `src/main.rs`): prints `<plan> <assessment> <target kind>` for every assessment of the
  assessors `nextest-run@1`, `package-tests@1`, `named-tests@1` and `capability-record@1` whose
  observation runs `task test-run` and whose profile, if given, is `default`; 22 on the tree
  (`slow-lane` is left out: its report is of the profile `slow`).
- `Taskfile.yml`: the task `quality-records` calls `inference-docs assess` once per listed
  assessment with `--from target/nextest/default/junit.xml`, writes
  `target/quality-records/<plan>/<assessment>.evidence.json`, and fails naming every refused
  assessment and on an empty list; the source is the forge run when the three `GITHUB_*`
  variables are set. `task check` runs it after `test-run`. The binary is built by `task
  docs-check` in `check-source`, so the task builds nothing new inside the gate.
- `.github/workflows/check.yml`: the step `task quality-records` in the full gate; for a push to
  `main` the upload of `target/quality-records/` as `quality-records-<commit>` with
  `actions/upload-artifact` pinned to the commit of its release v7.0.2.
- Tests: `crates/inference-docs/tests/quality/records.rs` (the list against the plan files, the
  task's commands run with stand-ins for `cargo` and `git`, the list filters),
  `crates/inference-docs/tests/workflows.rs` (both sides of the upload, the planning gate, no
  `continue-on-error`), `crates/inference-docs/tests/quality/assess.rs` (a record as the task
  writes it holds the record rules in the place of its own). The test of the quality documents
  is split into `problems()` (unchanged set of problems) and `record_problems()` (the rules of
  a record, one result per measure and per package, no page value).
- Pages: `quality/README.md` (the records of a check run, how to download them, that the
  documents are a gate and what fails it), `AGENTS.md`, `website/docs/guides/run-the-checks.md`.

Left open: no series of records is kept (the owner's open decision on where it lives); the page
values of `quality/` still name the check run of 2026-11-24 until the next story adopts a run on
`main`.
