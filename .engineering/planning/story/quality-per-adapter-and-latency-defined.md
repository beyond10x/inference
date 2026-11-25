---
format: aep.planning-md/3
id: story:quality-per-adapter-and-latency-defined
kind: story
status: implemented
title: Quality is defined per adapter, with latency and connect time
relations:
- serves: vision:inference
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: crates/inference-docs/tests/quality/adapters.rs
- confidence: cited
  path: crates/inference-docs/tests/quality/adversary_q2.rs
- confidence: cited
  path: crates/inference-docs/tests/quality/mod.rs
- confidence: cited
  path: quality/README.md
- confidence: cited
  path: quality/adapters.md
- confidence: cited
  path: quality/adapters.ratchet.yaml
- confidence: cited
  path: quality/assessors/named-tests@1.assessor.yaml
- confidence: cited
  path: quality/assessors/package-tests@1.assessor.yaml
- confidence: cited
  path: quality/assessors/printed-duration@1.assessor.yaml
- confidence: cited
  path: quality/gate.ratchet.yaml
- confidence: cited
  path: quality/not-measured.md
- confidence: cited
  path: quality/records
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T03:05:18Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T03:05:18Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-25T07:26:01Z", actor: "human:operator", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":1}}}
---
## Context

`quality/` defines the quality of the repository in Ratchet's formats at the commit
`f139eccc17a548444f0f4e1dc06520120c28739f`
(story:quality-defined-in-ratchet-formats). It measures the gate, the tests of the workspace as
one number, the documentation site and the resource profile of two local speech-to-text
adapters. It says nothing per adapter: a user who picks one of the eleven crates under
`adapters/` cannot read there how many tests hold it, which time bounds it keeps toward a
provider that does not answer, or how long it takes to connect and to give its first audio.

On 2026-11-25 the owner ordered the definition to go on with the individual adapters, latency
and connect time.

A read of the eleven crates on that day found:

- Every adapter's tests run in `task test-run`; the log of a `check` run names each test with
  its time.
- The time bounds an adapter keeps (open, connect, read and close timeouts, keep-alive
  intervals) are public constants, and tests of `task test-run` hold most of them against local
  stub servers. Some bounds have no test that fires them, and most default values are asserted
  by no test.
- Every committed claims or capability record is held by a test of its adapter and by
  `task docs-check`, which compare its `adapter_revision` with the version of the workspace.
- Latency has commands and no kept value: ignored live tests print a time to first audio, first
  transcript event or first token for some bindings, one prints the session start and close time
  of one binding, and the example `compare` of `realtime-voice` prints first audio, first
  transcript, response end and barge-in stop per provider and writes one JSON line per run. The
  hosted ones are billed. No task or workflow runs any of them and no file keeps their output.
- No command measures the connect time of any other binding, and the library has no field for
  it.

## Planned scope

- Per adapter crate: tests run and passed and its slowest test, read from the `check` run the
  page already names; each time bound that a test of `task test-run` holds, as an assertion that
  names the test; each committed record as an assertion that names the test holding it.
- Latency and connect time: each command that prints one, as a declared measure with its command,
  unit, direction and what the clock covers, and with no baseline: none of these commands was
  run for this story, and none is run by it.
- A gate for a release, if the format has a stage for it, that requires what the existing gates
  require at the commit a tag points at, and the assertions that tie the records and the
  documentation statuses to the version.
- `quality/not-measured.md`: what the read found missing, per binding.
- The check of `quality/` holds the new documents the way it holds the first ones, and goes red
  when an adapter crate has no entry.
- What a format cannot say goes to Ratchet as a finding; no field is invented here.

## Acceptance

- Every crate under `adapters/` has its measures on a page under `quality/`, each with a command
  of this repository, a unit, a direction and either a baseline with the run or file it was read
  from, or the statement that no run has produced one; the check goes red when a crate under
  `adapters/` has no entry (state transition: an adapter crate is added).
- A measure that asserts a time bound names the test that holds it, and the check goes red when
  that test is no longer in the file the measure names (state transition: the test is renamed).
- No latency or connect-time number stands in `quality/` that a named run or a file of the tree
  does not hold (state transition: the documents are added).

## Scope

As built, 2026-11-25. Commits `864f27355` (the unit) and `5117842f0` (what the adversary pass
found). No file under `adapters/` changed.

- `quality/adapters.md` with its plan `quality/adapters.ratchet.yaml`: 46 entries. Per adapter
  crate the tests run, the tests passed and the slowest test, read from the `check` run the page
  names; the time bounds a test of `task test-run` holds, each naming the test; the committed
  records, each naming the test that holds it. 26 entries are measures of latency or connect time
  and say `Baseline: none.`: each has its command, its unit, its direction and what the clock
  covers, and no number. None of these commands was run.
- `quality/README.md`: the format rules the page follows, among them that a named test is a
  function item and that no digit stands under an entry without a baseline.
- `quality/not-measured.md`: what the read found held by nothing, per binding: the connect
  timeouts no test fires, the defaults no test asserts, every `CLOSE_BOUND`, and connect time
  printed by one command only.
- `quality/assessors/`: three new declarations, `named-tests@1`, `package-tests@1` and
  `printed-duration@1` with 21 boundaries.
- `quality/records/`: 19 new records, 40 in all. `adapter-tests` counts 1166 tests executed and
  1166 passed over the eleven adapter crates.
- `quality/gate.ratchet.yaml`: 18 new requirements in gate `check`; 54 requirements in 5 gates.
- `crates/inference-docs/tests/quality/adapters.rs`, `mod.rs` and `adversary_q2.rs`: the check
  of `quality/`. It goes red when a crate under `adapters/` has no entry, when a named test is no
  longer a function item of the file its measure names, when a digit stands under an entry
  without a baseline, and when a measurement uses a boundary its assessor does not declare.
- `CHANGELOG.md`: one `Added` entry.

Not built: the gate for a release. Ratchet's formats cannot name the commit a tag points at, so
no such gate is declared; the finding went to Ratchet, which filed it as a draft. The assertions
that tie the documentation statuses to the version are in `quality/records/release-statuses`.

Follow-ups filed as drafts: story:connect-time-printed-per-binding,
story:connect-timeouts-fired-by-a-test, story:time-bound-defaults-and-close-bounds-held.
