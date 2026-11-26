---
format: aep.planning-md/3
id: story:quality-lists-name-the-tests-added-since-2026-11-24
kind: story
status: draft
title: The lists of the quality plans name the tests added since 2026-11-24, and a run on main holds them
relations:
- serves: vision:inference
revision: 1
---
## Context

The records of a check run hold results only for the tests their plan lists at that run's commit,
and the test of the quality documents refuses a committed record whose results differ from its
plan's list (`check_named_records`, `crates/inference-docs/tests/quality/adapters.rs`). So a change
of a list and the records that hold it cannot come from one run on `main`: the pull request that
changes the list fails against the committed records, and the run on `main` that would hold the
new list comes after it. Found while scoping story:quality-records-adopted-from-a-main-run: no
time-bounds record names the connect-timeout, close-bound and time-bound-default tests added
since 2026-11-24, and the list of `release-statuses` holds 4 tests where the note-status test
would be the fifth.

## Acceptance

- The lists of the plans of `quality/` name the tests added since 2026-11-24 that their measures
  are about (the connect-timeout, close-bound and time-bound-default tests per adapter, the
  note-status test in `release-statuses`), and the pages name them.
- The committed records that hold these lists come from a check run on `main`, adopted with the
  command of story:quality-records-adopted-from-a-main-run, and the way from the list change to
  that adoption is written in `quality/README.md` (for example: the check run uploads its JUnit
  report and the adoption reads it, or the list change lands with records marked as waiting for
  the run).
- The test of the quality documents passes at every commit of the change on `main`.

## Out of scope

- Latency, hosted calls, a series of records.
