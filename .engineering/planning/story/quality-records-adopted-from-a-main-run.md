---
format: aep.planning-md/3
id: story:quality-records-adopted-from-a-main-run
kind: story
status: active
title: The committed quality records are adopted from the records of a check run on main
relations:
- serves: vision:inference
revision: 3
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
  records of one named check run on `main`, byte for byte as the artifact holds them except for the
  fields the record rules of `quality/README.md` say are set at adoption; each names that run as
  its source.
- A task (or `inference-docs` command) adopts the records of an artifact of a run on `main`,
  given the run id: it downloads nothing itself (the artifact is a directory given to it), refuses
  an artifact of a run that is not on `main` or not of the workflow `check`, refuses a record that
  `inference-docs assess` would refuse, and writes nothing on a refusal.
- The pages name the tests of that run: `quality/adapters.ratchet.yaml`, `quality/adapters.md`,
  the runs table of `quality/README.md` and `quality/not-measured.md` agree with the adopted
  records, and the test of the quality documents passes.
- The `release-statuses` list holds the tests the run held, the note-status test among them.
- The records that no artifact holds (the step, job, scan and docs records) stay as they are and
  the README says which those are.

## Out of scope

- Latency and connect-time measures, hosted calls, a series of records (the owner's decision A of
  the evaluation plan), the records of a pull-request run.
