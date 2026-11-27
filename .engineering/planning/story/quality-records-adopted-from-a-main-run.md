---
format: aep.planning-md/3
id: story:quality-records-adopted-from-a-main-run
kind: story
status: active
title: The committed quality records are adopted from the records of a check run on main
relations:
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T07:09:12Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T07:09:13Z", actor: "human:operator", revision: 3}
---
## Context

`quality/records/*.evidence.json` are still the records written by hand from the check runs of
2026-11-24 (`quality/README.md`, "The records under `records/` are still the ones written by
hand"). Since story:quality-records-written-by-each-check-run, every push to `main` uploads the
records `task quality-records` wrote as the artifact `quality-records-<commit>`; the first is
`quality-records-0ac1e2f7b74f60b8019a25e5a98bc2460841ecd7` of the check run 45771353687 on `main`
(22 records). The lists of tests the pages name change only from a run on `main` (the owner's rule
for `quality/`), so the tests added since 2026-11-24 (connect timeouts, close bounds, the drop
tests, the ignore check) are named by no page yet, and the list of `release-statuses` holds four
tests where the run has five (the note-status test).

## Acceptance

- The 22 committed records under `quality/records/` that `task quality-records` writes are the
  records of the check run 45771353687 on `main` (commit `0ac1e2f7b74f60b8019a25e5a98bc2460841ecd7`),
  byte for byte as its artifact `quality-records-<commit>` holds them; each names that run as its
  source.
- `inference-docs` has a command, called by a task, that adopts the records of an artifact of a
  run on `main` into `quality/records/`. It takes the directory the artifact was downloaded to and
  the output of `gh run view <id> --json databaseId,headBranch,headSha,event,workflowName` saved by
  the caller; it downloads nothing. It refuses, naming the reason and writing nothing: a run that
  is not a push to `main` of the workflow `check`, a run id or commit the records do not name, a
  missing or extra assessment against `inference-docs junit-assessments`, a record of another
  project, assessor or class than `ci`, a record whose source `assess` would refuse, and a record
  the record rules of the quality documents refuse. It writes all files or none.
- The pages agree with the adopted records: the runs table of `quality/README.md` gains the run;
  `quality/README.md`, `quality/adapters.md` and `quality/not-measured.md` state the values of the
  adopted records and name their run, and the test of the quality documents passes. Facts no
  record holds (the skipped count, the stack test, the doc tests) name the run whose log they come
  from.
- `quality/README.md` names the 18 records no artifact holds (the steps, jobs, scans, the checks of
  `task check-source`, the slow lane and the docs) and that they stay records written by hand.
- The lists of tests in the plans do not change in this story: a record of a run holds the tests
  its plan listed at that run's commit.

## Out of scope

- Latency and connect-time measures, hosted calls, a series of records (the owner's decision A of
  the evaluation plan), the records of a pull-request run.

## Scope

Derived 2026-11-26 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `quality/` (the 22 records, `README.md`, `adapters.md`, `not-measured.md`) — cited
- **Files:** `quality/records/{adapter-tests,workspace-tests,release-statuses,sherpa-onnx-stt-record,whispercpp-stt-record}.evidence.json` and the 17 `*-time-bounds` / `*-records` files of the nine adapters — cited (the 22 files of artifact `quality-records-0ac1e2f7…` of run 45771353687)
- **Files:** `quality/README.md` lines 102, 104-126, 128-210, 218, 239-246, 255-266, 382, 384-390, 689, 723, 778 (779-793 if the list grows) — cited
- **Files:** `quality/adapters.md` lines 8-9, 21-42, the 33 table rows of the per-crate tables (85-87, 113-115, 165-167, 227-229, 308-310, 360-362, 461-463, 609-611, 736-738, 782-784, 800-802), and the `Baseline:` lines 97, 124, 148, 176, 195, 238, 271, 319, 343, 371, 408, 472, 485, 620, 662, 747, 766 — cited
- **Files:** `quality/not-measured.md` lines 212, 499-500, 596 — cited
- **Files:** `crates/inference-docs/src/main.rs` (`enum Action`, a new subcommand beside `Assess` and `JunitAssessments`) — cited
- **Files:** `crates/inference-docs/src/assess.rs` (`junit_assessments`, `source`, `target`, `CLASSES`, `sha256`: the refusals reuse them) — cited
- **Also likely:** a new `crates/inference-docs/src/assess/adopt.rs` — inferred
- **Files:** `Taskfile.yml` (a new task beside `quality-records`) — inferred
- **Files:** `crates/inference-docs/tests/quality/adapters.rs:735-739,747-768` and `:951-967`, `adversary_q1.rs:209-213`, `assess.rs:25-27,261,352-394` — cited (they plant or expect the values of today's records)
- **Also likely:** a new `crates/inference-docs/tests/quality/adopt.rs` with `mod adopt;` in `tests/quality/mod.rs`, and fixture artifacts under `crates/inference-docs/tests/fixtures/adopt/` — inferred
- **Also likely:** `quality/gate.ratchet.yaml:261-265` (the `release-statuses` list) and the `tests` lists of `quality/adapters.ratchet.yaml` — inferred, only if a later run is adopted (option A defers them)
- **Also likely:** `AGENTS.md:44,58` (the table row of `.github/workflows/` and the Commands block) — inferred
- **Also likely:** `.github/workflows/check.yml:151-157` — inferred, only under option B or if run metadata is added to the artifact
- **Documents:** the three pages of `quality/`, `AGENTS.md` — cited
- **Confidence:** high for `quality/` and the tests, because the artifact was compared field by field with the committed records; medium for the command's location and shape, because the story names no symbol for it
- **Would collide with:** any unit touching `quality/` (records, pages, plan lists), `crates/inference-docs/tests/quality/`, the `Action` enum of `crates/inference-docs/src/main.rs`, or the `quality-records` area of `Taskfile.yml`
- **Safety fact:** the 22 artifact records have the same key order, layout and structure as the committed ones. The raw diff and the `jq -S` diff have equal line counts for every file, and no JSON path is added or removed. So copying them changes only values, and no record rule of `record_problems()` other than the page values can break — step 4 (jq path diff run against the real files) for the records; unproven that `problems()` passes after the pages change
