---
format: aep.planning-md/3
id: story:quality-records-written-by-a-program
kind: story
status: active
title: A program writes the record of an assessment from what a run left behind
relations:
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T14:04:59Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T14:04:59Z", actor: "human:operator", revision: 3}
---
## Context

`quality/` defines the measures of this repository in the formats of Ratchet. Nothing evaluates
them: no assessor declaration has a program, the records under `quality/records/` were written by
hand from four CI runs of 2026-11-24, and the measures of latency and connect time have no value.
The owner wrote on 2026-11-25: "we should not only define these KPIs and metrics and stuff. When
that is merged, we also want to evaluate it, right, and track over time."

A scope read of 2026-11-25 (by reading, no run) found:

- Ratchet at its release `0.0.1` evaluates nothing: every command answers that it is not built.
  Its specification says how an assessor is called (`--project --assessment --root --class --out
  --revision` and the assessor's inputs; exit 0 with a document, 1 for refused inputs, anything
  else for a stopped run) and what a `ratchet-evidence/1` document holds.
- Of the 90 measures of the three plan files, 58 can be read from a forge run of an existing
  workflow, 16 from a local command without a hosted call, 16 from a command that calls a hosted
  provider.
- The values of a `check` run are in what `cargo nextest run` leaves behind: the JUnit report
  `target/nextest/default/junit.xml` (tests run and passed and the slowest test, per package and
  for the workspace; the named tests of the time bounds and of the records). After
  `task test-run` that file holds the last part only, the stack test, which overwrites the report
  of the workspace run.
- The commands of the latency measures print a Rust `Duration` in debug form, some inside
  `Some(...)`.
- `crates/inference-docs` is the repository's tool crate (`publish = false`); its test under
  `tests/quality/` already reads the plan files, the declarations and the records and refuses a
  record that disagrees with them.

This story builds the program that turns what a run left behind into a record. It runs no
measure and keeps no history; the stories after it do.

## Acceptance

Given the plan files and the assessor declarations of `quality/` and a file that a run left
behind (a JUnit report of cargo-nextest, or the captured output of a command that prints a
duration), when `inference-docs assess` is called for one assessment with that file, a revision
and a source, then it writes one `ratchet-evidence/1` document that the test of the quality
documents accepts in the place of that assessment's record, with the values the file holds; and
when the file does not hold what the declaration names, it writes no document and exits 1.

## Planned scope

- A subcommand `assess` of `crates/inference-docs`, called as Ratchet's specification calls an
  assessor, plus the file to read and the source to name.
- A reader of the JUnit report for the assessors `nextest-run`, `package-tests`, `named-tests`
  and `capability-record`.
- A reader of printed durations for the assessor `printed-duration`: one value per run.
- Tests on fixtures only: a report and an output written for the test, the document compared
  field by field, the refusals, and one test that puts a written document in the place of a
  committed record and expects the test of the quality documents to find no problem.
- The step that keeps the JUnit report of the workspace run from being overwritten, if the reader
  needs it: a change of `Taskfile.yml` or `.config/nextest.toml`.
- The sentences of `quality/README.md` that say no assessor has a program.

## Out of scope

- Running a measure's command: the program reads a file.
- A workflow change, an uploaded artifact, a history: the stories after this one.
- A change of a committed record, a plan file, an entry state of `quality/adapters.md`.
- The readers of forge job and step times, of an exit status, and of the `runs.jsonl` of the
  `compare` example.
- A call to a hosted provider, a local model run, a benchmark.
- A new crate.

## Scope as built

Built on the branch `quality/record-program`, in `crates/inference-docs` (not published):

- The command `inference-docs assess`: Ratchet's six arguments of an assessor call and five of
  its own (`--from`, `--source`, `--started-at`, `--ended-at`, `--exit-status`). It writes one
  `ratchet-evidence/1` document or refuses; `quality/README.md` says the exit codes and the call.
  Files: `src/assess.rs`, `src/assess/junit.rs`, `src/assess/printed.rs`, `src/main.rs`.
- Readers of a JUnit report of cargo-nextest for the assessors `nextest-run@1`, `package-tests@1`,
  `named-tests@1` and `capability-record@1`. A named test is found by the module path of its
  file, the inline modules around its function and its name; the counts read are compared with
  the report's own.
- A reader of the captured output of a test command for `printed-duration@1`, 12 printed labels.
  A pass is taken only from the closing line of the tests that printed the measures.
- Not read, exit 2 with the reason: `compare`, `elevenlabs-live-example`,
  `cascade-local-adapters` (more than one value per run) and `cascade-talk` (its exit status does
  not tell a completed run from a failed one).
- `quality/assessors/nextest-run@1.assessor.yaml` no longer lists `skipped`, and the record of
  `workspace-tests` holds 0 there: the JUnit report of cargo-nextest 0.9.143 holds no ignored
  test.
- `Taskfile.yml` and `.config/nextest.toml`: the stack test runs under the profile `stack`, so the
  default report stays the one of the workspace run.
- Tests: `crates/inference-docs/tests/quality/assess.rs` with fixtures under
  `tests/fixtures/assess/`; the package runs 106 tests.

Left open, each named in `quality/README.md`: a call through Ratchet as its specification stands
cannot pass the five own arguments; the document cannot know the cargo-nextest profile of the
report it is given; the committed records are still the ones written by hand.
