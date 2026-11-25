---
format: aep.planning-md/3
id: story:quality-per-adapter-and-latency-defined
kind: story
status: active
title: Quality is defined per adapter, with latency and connect time
relations:
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T03:05:18Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T03:05:18Z", actor: "human:operator", revision: 3}
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
