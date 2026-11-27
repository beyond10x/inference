---
format: aep.planning-md/3
id: story:quality-records-series-on-a-ref
kind: story
status: active
title: Each main run's quality records are appended once to a series on a ref CI alone writes
relations:
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-27T10:16:47Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-27T10:16:47Z", actor: "human:operator", revision: 3}
---
## Context

Every push to `main` uploads the records `task quality-records` wrote as the artifact
`quality-records-<commit>` (story:quality-records-written-by-each-check-run). An artifact is kept
for a limited time, and a run cancelled by the concurrency of `check` leaves none, so the values
of a measure over time exist nowhere. Ratchet's design keeps them as a series: one job on the
trusted branch appends each handed-over record once, under its id, to a ref it alone writes
(`beyond10x/ratchet`, `docs/design.md`).

The owner decided on 2026-11-27 (D48 A) that the series is written by CI with `GITHUB_TOKEN`.
The ruleset of this repository lets only the bot app create or update a branch, so the series
lives on a ref outside `refs/heads` (decision D65; see the approval record of this wave).

## Planned scope

- A workflow `.github/workflows/quality-series.yml`, triggered by `workflow_run` of `check`
  completed with success on a push to `main`. One job, the only writer of the series, with
  `contents: write` and `actions: read` on that job alone and a concurrency group that queues
  (never cancels) its runs. It downloads the artifact `quality-records-<head sha>` of that run,
  fetches the series ref (`refs/ratchet/records`, named in one place of the workflow), appends,
  commits as the Actions identity and pushes without force. A refused push fails the job.
- `inference-docs series-append --from <artifact dir> --series <checkout of the series>`: writes
  each record that is not in the series yet to `<plan>/<assessment>/<sha256 of its bytes>.evidence.json`
  byte for byte, never changes or removes a file, and prints what it added; a record that is
  there already is skipped; a file of the artifact that is not a record of a known assessment is
  refused and nothing is written.
- `inference-docs history --series <dir> <assessment> [<measure>]` and
  `task quality-history ASSESSMENT=… [MEASURE=…]`: fetch the ref into a directory under `target/`
  and print one line per record, oldest first by the record's revision time or run, with its
  run, revision and the measure's value.
- Tests in `crates/inference-docs` over fixtures: append adds, skips and refuses as above; the
  history lists in order; `tests/workflows.rs` checks the workflow's trigger, its permissions,
  its concurrency and that no other workflow writes the ref.
- `quality/README.md` and `AGENTS.md`: where the series is and how to read it.

## Acceptance

- Appending the same artifact twice adds its records once; a second artifact adds only records
  whose bytes differ; no file of the series is ever changed (state transition: the series gains
  one file per new record).
- `task quality-history` lists a measure's values in order from a series directory (state
  transition: none; read only).
- The workflow test refuses a write permission on any other job, a trigger other than a
  successful `check` on `main`, and a concurrency that cancels (state transition: none).
- The first push to `main` after the merge appends that run's records to the ref, or the job
  fails naming the refused push (state transition: the ref exists on the forge).

## Out of scope

Gate verdicts and views over the series; retention; local records (they stay in
`.ratchet/records`); records of `slow` and Docs UX; adopting records into `quality/records/`
(story:quality-records-adopted-from-a-main-run).
