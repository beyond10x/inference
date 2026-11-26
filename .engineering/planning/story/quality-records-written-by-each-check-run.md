---
format: aep.planning-md/3
id: story:quality-records-written-by-each-check-run
kind: story
status: active
title: Each check run writes the quality records its JUnit report holds and keeps those of main as an artifact
relations:
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T03:26:52Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T03:26:52Z", actor: "human:operator", revision: 3}
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
`ratchet-evidence/1` document per assessment that reads the JUnit report exists under
`target/quality-records/`, each accepted by the test of the quality documents in the place of
that assessment's record, and the task exits non-zero and names the assessment when the report
does not hold what an assessment names; and when the workflow `check` ran for a push to `main`,
then its run has one artifact that holds those documents and names the commit.

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
