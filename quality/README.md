# The quality of this repository

This directory says what counts as the quality of `inference`, what each value is today, and
where the value comes from. The measures of each adapter crate are on
[adapters.md](adapters.md): its tests, its time bounds, its records and the commands that print
a latency. What has no command that produces it is in [not-measured.md](not-measured.md).

The documents are written in the formats of Ratchet (`beyond10x/ratchet`), against its
specification at commit `f139eccc17a548444f0f4e1dc06520120c28739f`.

- No Ratchet command works at that commit. Nothing here is run by Ratchet.
- Every field name is a proposal of that specification. A release of Ratchet can change it.
- The documents add no gate to the repository and change none. They describe what `task check`,
  `task test-slow` and the workflows under `.github/workflows/` already enforce.
- A measure can be defined and have no value. The latency measures on
  [adapters.md](adapters.md) are such: each names its command and its clock, and says
  `Baseline: none.`

## The files

| File | Format | Holds |
| --- | --- | --- |
| `../ratchet.toml` | local config | the specification files and the assessor directory |
| `gate.ratchet.yaml` | `ratchet/1` | the gate, the tests, the specification, turnaround, the release statuses |
| `docs-site.ratchet.yaml` | `ratchet/1` | the documentation site and the turnaround of its workflow |
| `adapters.ratchet.yaml` | `ratchet/1` | per adapter crate: tests, time bounds, records, latency; the resource profile of the local speech-to-text adapters |
| `assessors/<name>@<version>.assessor.yaml` | `ratchet-assessor/1` | what each assessor reports |
| `records/<assessment>.evidence.json` | `ratchet-evidence/1` | one baseline per assessment that has one, read from a CI run |

Three pages say the same for a reader: this one, [adapters.md](adapters.md) and
[not-measured.md](not-measured.md).

No assessor has a program. Each declaration is without `command`: it names the outputs, units
and scopes that a program would report. The records were written by hand from the CI runs named
below.

## The test that keeps the documents true

`crates/inference-docs/tests/quality/`, part of `task test-run`: `mod.rs` and, for the rules
about adapters, tests and commands, `adapters.rs`. It is not `ratchet validate`. It fails when:

- a file stops parsing, has another format or names another project, a name is declared in two
  files, an assessor declaration or a record is not in the file named after it, or
  `ratchet.toml` does not list the specification files and the assessor directory;
- an assessment names a subject or an assessor that is not declared, a measurement names an
  output its assessor does not declare or a boundary its assessor does not declare for that
  output, a requirement names an assessment or a measurement that is not declared, or a gate
  requires a requirement that is not declared;
- a record names an assessment that is not declared, an assessor other than the one of its
  assessment, or an output that assessor does not declare;
- the coverage of a record holds another number than 0 for a count its assessor does not
  report;
- an assessment names packages as its input, and its record has `tests_run` results for
  another set of packages, or its coverage counts another number of executed tests than the
  `tests_run` results sum to, or of passed tests than the `tests_passed` results sum to;
- the unit of a measurement, of the condition of a requirement or of a result in a record
  differs from the unit the assessor declares for the output;
- a command calls a task `Taskfile.yml` does not have;
- a command calls no task, stands in no `cmds` list of `Taskfile.yml` and names no test or
  example of a package, unless it is the one command below, or that list names a command that
  is not such a one;
- a `cargo test` command that names a package with `-p` and a test with `--test`, or a
  `cargo run` command that names a package and an example with `--example`, in the YAML or in a
  code span of one of the three pages, names a package that is no member of the workspace, a
  test or example whose file does not exist, or a feature the `Cargo.toml` of the package does
  not declare, or it leaves out a feature the target requires;
- a measure has no entry on this page or on [adapters.md](adapters.md), an entry has no measure
  or stands twice, or an entry has no `Baseline:` line that names a run of the table below or a
  file of the tree;
- an entry names a run, and `records/` has no record of that run for its assessment with a value
  for the measure;
- a baseline or a table differs from the values in the record, or a bound differs from the
  number in the requirement;
- an entry says `Baseline: none.` in other words than `none. No run of this command is kept.`,
  or a record holds a value for its measure, or a requirement bounds its measure;
- anything under the heading of such an entry, up to the next heading, holds a digit: a line of
  the entry other than `Command:` and `Needs:`, a bullet without a name, a paragraph, a row of
  a table;
- a directory under `adapters/` with a `Cargo.toml` has no section on
  [adapters.md](adapters.md), a section names no such directory or stands twice, or the sections
  are not in the order of the directories;
- a section does not name a file committed under `records/` or `capability/` of its adapter;
- the table of a section lacks a row for a measure with one value per package, or a row differs
  from the record: the value, the unit, the test, or the run it was read from;
- one of the three pages names a test, by its function and its file, and the file does not
  exist or has no line that starts a function of that name: `fn`, the name and `(`, with
  nothing before them but `pub`, `async`, `const` or `unsafe`. The name in a comment or in a
  string does not count;
- an assessment lists tests as its input, and one is not in its file, the entry of the measure
  names another set of tests, or the record has another set of results;
- an entry on [adapters.md](adapters.md) does not give the command of its assessment, whole, in
  its `Command:` line.

One command stands in no `cmds` list of `Taskfile.yml` and names no test or example of a
package, so the test holds it against nothing. A change of the workflow step that runs it goes
unnoticed here:

- `npx playwright test --workers=2`

The test reads no CI run. That a record equals its run was checked by hand. A baseline without
a record is held only to name a file that exists: the test does not read the number in that
file. A bound without a number ("must hold", "recorded, no bound") is not held against
anything. Of the other lines of an entry, the test holds the tests and the commands they name,
and nothing else: not the sentence for a user, not the value of a constant, not what a line
says a test compares, not the clock of a latency, not whether a command is billed. That a named
test is ignored, or runs in `task test-run`, is not held either.

More that the test does not hold:

- The search for a named test takes any function of the name: one without `#[test]`, one with
  `#[ignore]`, and one in a file behind a `#![cfg(feature = "...")]` that the gate does not
  turn on. A line of a block comment, or of a string over several lines, that starts like the
  function is taken for it.
- A number written in words under an entry that says `Baseline: none.`. The test finds digits
  only.
- The coverage of a record beyond the two rules above: not that a count is the count of the
  run.

## The runs the baselines were read from

All ran on 2026-11-24. The first three ran on `main` at
`59b8f4a858ab9c4eef94bc21de8e7bb85f90ddad`.

| Name here | Workflow | Run |
| --- | --- | --- |
| check run | `check` | [45474241015](https://github.com/beyond10x/inference/actions/runs/45474241015) |
| slow run | `slow` | [45474240955](https://github.com/beyond10x/inference/actions/runs/45474240955) |
| gates run | `Shared source gates` | [45474241551](https://github.com/beyond10x/inference/actions/runs/45474241551) |
| docs run | `Docs UX` | [45467764733](https://github.com/beyond10x/inference/actions/runs/45467764733) |

`Docs UX` does not run on `main`. The docs run is of a pull request with the head
`adf479ee2707962b93a2c8ce6509bb53775024bb`. It checked out the merge of that head into `main`,
the commit `1ed5587d63ab46006e85cc0164e7e0ecd70ec18c`. The tree of that commit is the tree of
`59b8f4a8`: `17ea6f2eb4d9601ed4d3bd6c5e33c966dfe225f0`. The records of the docs run name the
head as the candidate and the merge commit as the revision, with the selection `merge_result`.

The values per adapter crate on [adapters.md](adapters.md) were counted in the log of the step
`task test-run` of the check run. That log has one line per test, with the result, the time in
brackets and the name of the package. This counts the tests of one package, here
`inference-aws`, in a log saved as the file `$log`:

```sh
sed -E 's/(\x1b|\^\[)\[[0-9;]*m//g' "$log" |
  grep -cE '\( *[0-9]+/3059\) +inference-aws(::[^ ]+)? '
```

The same lines that say `PASS` are the tests that passed, and the longest time on them is the
slowest test.

## How to read an entry

The heading is the measure: `<assessment>/<measurement>`, as the YAML names it. "Holds" means
the measure is an assertion: it has no unit, and its direction is that it must hold. "Recorded,
no bound" means nothing fails when the value gets worse. The baseline ends with its source: a
run of the table above, or a file of the tree.

A test is named by the word "test", its function in a code span, the word "of", and its file
in a code span, as a path from the root of the repository. An entry that says
`Baseline: none. No run of this command is kept.` defines a measure that has no value yet.
## Source

### `formatting/clean`

- For a user: the code reads the same everywhere, so a patch shows only what it changes.
- Command: `cargo fmt --all --check`, a step of `task check-source`.
- Unit and direction: none; holds.
- Bound: must hold. `task check-source` fails otherwise.
- Baseline: holds. Check run.

### `lint/clean`

- For a user: no code that Clippy warns about ships in a release.
- Command: `cargo clippy --workspace --all-targets --all-features -- -D warnings`, a step of
  `task check-source`.
- Unit and direction: none; holds.
- Bound: must hold. `-D warnings` makes every warning an error.
- Baseline: holds. Check run.

### `doc-build/clean`

- For a user: the API documentation builds and none of its links is broken.
- Command: `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps`, a step of
  `task check-source`.
- Unit and direction: none; holds.
- Bound: must hold. `-D warnings` makes every warning an error.
- Baseline: holds. Check run.

### `generated-docs/current`

- For a user: the reference pages of the site say what the code of the same commit does.
- Command: `task docs-check`.
- Unit and direction: none; holds, once per generated file.
- Bound: must hold for every file. `task check-source` fails otherwise.
- Baseline: holds for 6 of 6 files. Check run.

### `planning-store/valid`

- For a user: the stories and decisions behind a release can be read and are consistent.
- Command: `aep plan artifact validate`, a step of `task check-source`.
- Unit and direction: none; holds.
- Bound: must hold. `task check-source` fails otherwise.
- Baseline: holds. Check run.

### `source-scan/passed`

- For a user: no secret and no private detail is in the history of the public repository.
- Command: the workflow `.github/workflows/shared-gates.yml`, job `common / Security and
  privacy`. It calls a workflow of `beyond10x/gates`. No task of this repository runs it.
- Unit and direction: none; holds.
- Bound: must hold. The job fails otherwise.
- Baseline: holds. Gates run.

## Specification and conformance

### `ess-specification/valid`

- For a user: the behaviour the library promises is written down in a form a tool can check.
- Command: `ess specify validate --path ess`, a step of `task check-source`.
- Unit and direction: none; holds.
- Bound: must hold. `task check-source` fails otherwise.
- Baseline: holds, over 11 files. Check run.

### `ess-specification/scenarios`

- For a user: how many scenarios were written by hand beside those derived from the rules.
- Command: `ess specify validate --path ess`, which prints the count.
- Unit and direction: scenarios; higher is better.
- Bound: recorded, no bound.
- Baseline: 10 scenarios. Check run.

### `conformance/passing-scenarios`

- For a user: each promised behaviour has a scenario, and the library passes it.
- Command: `cargo test -p inference-testkit --features run --test conform`, part of
  `task test-run`. It writes `report.json` under `target/tmp/conform-<run>/`.
- Unit and direction: none, it is a set of scenario ids; no id may leave the set.
- Bound: every id that `crates/inference-testkit/tests/conform.rs` names must pass, and none may
  fail. The test fails otherwise.
- Baseline: the 472 ids named in `crates/inference-testkit/tests/conform.rs`, with no skipped id
  in `ess/SKIPPED.md`.

The test that holds those ids passed in the check run. The total the suite executed is not in
that run: see [not-measured.md](not-measured.md). There is no record for this assessment, so the
baseline names the two files and no run.

### `native-archive/checksum-matches`

- For a user: the native library a local adapter links is the one that was reviewed.
- Command: `task sherpa-onnx-archive`.
- Unit and direction: none; holds.
- Bound: must hold. The task fails when the sha256 of the download differs from the pin in
  `Taskfile.yml`.
- Baseline: holds, against the pin
  `bf2d998c8b07012cd5098f3b92673bc1333fd9b927767d7cb664be8190d8bc0b`. Check run.

## Tests

### `workspace-tests/slowest-test`

- For a user: the tests are quick enough that every change runs all of them.
- Command: `task test-run`, the part that runs `cargo nextest run` with the default profile.
- Unit and direction: seconds; lower is better.
- Bound: under 60 s. `.config/nextest.toml` ends a test at 60 s and counts it as failed.
- Baseline: 8.011 s. Check run.

The same run gives the coverage of this assessment, which the record holds: 3059 tests run,
3059 passed, 72 skipped. Recorded, no bound. `task test-run` has two more parts that this
assessment does not cover: the stack test (23 tests run, 23 passed) and the doc tests (38
passed). Check run.

The tests run and passed and the slowest test of each adapter crate, from the same part of the
same run, are on [adapters.md](adapters.md).

### `slow-lane/slowest-test`

- For a user: a wrong use of a macro of the library gives the compile error the tests pin.
- Command: `task test-slow`.
- Unit and direction: seconds; lower is better.
- Bound: under 600 s. The `slow` profile of `.config/nextest.toml` ends a test after ten periods
  of 60 s.
- Baseline: 26.703 s, with 1 test run and 1 passed. Slow run.

The slow lane does not run for every change. On a workstation `task test-slow` runs only when
one of the nine `sources` of the task in `Taskfile.yml` changed since its last run. The workflow
`.github/workflows/slow.yml` runs it for a change under the same nine paths, and for a change of
`Taskfile.yml` or of the workflow file itself. `task check` does not run it. The gate `slow` of
the YAML carries the list of the workflow and qualifies runs in CI only. The list of the task is
not in the YAML: it is compared with the last local run, not with the change.

## Turnaround

### `test-run-step/run-time`

- For a user: a fix reaches a release sooner when the test step is short.
- Command: `task test-run`.
- Unit and direction: seconds; lower is better.
- Bound: under 120 s. `Taskfile.yml` ends the step at 120 s and fails it.
- Baseline: 101 s. Check run, step `task test-run`.

### `check-source-step/run-time`

- For a user: none directly; it is part of the time a change waits for its verdict.
- Command: `task check-source`.
- Unit and direction: seconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 224 s. Check run, step `task check-source`.

### `test-build-step/run-time`

- For a user: none directly; it is part of the time a change waits for its verdict.
- Command: `task test-build`.
- Unit and direction: seconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 233 s. Check run, step `task test-build`.

### `check-workflow/run-time`

- For a user: none directly; it is the time a change waits for its verdict once a runner has it.
- Command: the workflow `.github/workflows/check.yml`, job `inference / task check`.
- Unit and direction: seconds; lower is better.
- Bound: under 2700 s. The job has `timeout-minutes: 45`.
- Baseline: 589 s. Check run.

### `check-workflow/queue-wait`

- For a user: none directly; it is the time a change waits for a runner.
- Command: the workflow `.github/workflows/check.yml`, job `inference / task check`.
- Unit and direction: seconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 284 s, from the creation of the run to the start of the job. Check run.

### `slow-lane-step/run-time`

- For a user: none directly; it is part of the time a change of a macro waits for its verdict.
- Command: `task test-slow`.
- Unit and direction: seconds; lower is better.
- Bound: recorded, no bound of its own. Neither the task nor its step in
  `.github/workflows/slow.yml` has a limit. What ends a long run is the bound of each test and
  the timeout of the job.
- Baseline: 76 s. Slow run, step `task test-slow`.

### `slow-workflow/run-time`

- For a user: none directly; it is the time a change of a macro waits for the slow lane once a
  runner has it.
- Command: the workflow `.github/workflows/slow.yml`, job `inference / task test-slow`.
- Unit and direction: seconds; lower is better.
- Bound: under 1800 s. The job has `timeout-minutes: 30`.
- Baseline: 105 s. Slow run.

## Documentation site

Twelve measures of `docs-ux` say what a reader of the site meets. They share what follows.

- Command: `npx playwright test --workers=2` under `ux/`, then `node tools/ratchet.ts`, which
  compares with `ux/baseline.json`. No task runs them; the workflow
  `.github/workflows/docs-ux.yml` does, on a pull request that touches `website/`, `docs/` or
  `ux/`.
- Direction: lower is better.
- Bound: no value may be worse than its value in `ux/baseline.json`, and a better value must be
  written to that file in the same change. `node tools/ratchet.ts` fails otherwise.
- Baseline: each value below is the value in `ux/baseline.json`. The docs run measured the same:
  45 tests passed, 80 values, 0 problems.
- A view is a scenario at a viewport, written `<scenario>/<viewport>`. The viewports are
  1440x900 and 390x844.

The first five are measured over every page and theme of the built site, scenario `sitemap`.

### `docs-ux/horizontal-scroll`

- For a reader: no page scrolls sideways.
- Unit: pages.
- Baseline: 0 at both viewports. Docs run.

### `docs-ux/overflowing-elements`

- For a reader: nothing is cut off at the edge of the screen.
- Unit: elements.
- Baseline: 0 at both viewports. Docs run.

### `docs-ux/always-visible-misses`

- For a reader: what must always be on screen is on screen.
- Unit: misses.
- Baseline: 0 at both viewports. Docs run.

### `docs-ux/accessibility-violations`

- For a reader: a screen reader meets no serious or critical violation.
- Unit: nodes.
- Baseline: 0 at both viewports. Docs run.

### `docs-ux/keyboard-unreachable`

- For a reader: everything that can be clicked can be reached with the keyboard.
- Unit: elements.
- Baseline: 0 at both viewports. Docs run.

The other seven are measured for five tasks a reader has: find the dependency lines, the error
kinds, the limitations, the provider comparison, the variable of the realtime model.

### `docs-ux/completion-misses`

- For a reader: the task ends on the page that answers it.
- Unit: misses.
- Baseline: 0 in all ten views. Docs run.

### `docs-ux/steers`

- For a reader: the reader never has to go back a page.
- Unit: steers.
- Baseline: 0 in all ten views. Docs run.

### `docs-ux/actions`

- For a reader: how many clicks the task takes, with opening the menu and going back.
- Unit: actions.
- Baseline: the table. Docs run.

| Scenario | 1440x900 | 390x844 |
| --- | --- | --- |
| find-dependency-lines | 1 | 2 |
| find-error-kinds | 2 | 4 |
| find-limitations | 2 | 4 |
| find-provider-comparison | 1 | 2 |
| find-realtime-model-variable | 1 | 2 |

### `docs-ux/looks`

- For a reader: how often the next link is off screen and has to be scrolled to.
- Unit: looks.
- Baseline: the table. Docs run.

| Scenario | 1440x900 | 390x844 |
| --- | --- | --- |
| find-dependency-lines | 0 | 0 |
| find-error-kinds | 1 | 1 |
| find-limitations | 1 | 1 |
| find-provider-comparison | 0 | 1 |
| find-realtime-model-variable | 1 | 1 |

### `docs-ux/steps-to-act`

- For a reader: how many links the reader follows to the page that answers the task.
- Unit: steps. A value within 5 percent of the baseline counts as equal.
- Baseline: the table. Docs run.

| Scenario | 1440x900 | 390x844 |
| --- | --- | --- |
| find-dependency-lines | 1 | 1 |
| find-error-kinds | 2 | 2 |
| find-limitations | 2 | 2 |
| find-provider-comparison | 1 | 1 |
| find-realtime-model-variable | 1 | 1 |

### `docs-ux/glance-cost-median`

- For a reader: how hard the next link is to find, at the median step of the task.
- Unit: level. 1 is a link in the main navigation on screen, 2 a link elsewhere on screen, 3 a
  link off screen.
- Baseline: the table. Docs run.

| Scenario | 1440x900 | 390x844 |
| --- | --- | --- |
| find-dependency-lines | 1 | 1 |
| find-error-kinds | 1 | 1 |
| find-limitations | 1 | 1 |
| find-provider-comparison | 1 | 1 |
| find-realtime-model-variable | 3 | 1 |

### `docs-ux/glance-cost-p95`

- For a reader: the same, at the hardest step of the task.
- Unit: level. 1 is a link in the main navigation on screen, 2 a link elsewhere on screen, 3 a
  link off screen.
- Baseline: the table. Docs run.

| Scenario | 1440x900 | 390x844 |
| --- | --- | --- |
| find-dependency-lines | 1 | 1 |
| find-error-kinds | 3 | 3 |
| find-limitations | 3 | 3 |
| find-provider-comparison | 1 | 3 |
| find-realtime-model-variable | 3 | 3 |

## Documentation site: turnaround

The four measures below are the times of the same tests. Each is from the one docs run.

### `docs-ux/slowest-test`

- For a reader: none directly; the tests of the site are quick enough to run for every change
  of it.
- Command: `npx playwright test --workers=2` under `ux/`, which prints its slowest tests.
- Unit and direction: seconds; lower is better.
- Bound: under 60 s. `ux/playwright.config.ts` ends a test at 60 s (`timeout: 60_000`).
- Baseline: 4.3 s. Docs run.

### `docs-ux/run-time`

- For a reader: none directly; it is part of the time a change of the site waits for its
  verdict.
- Command: `npx playwright test --workers=2` under `ux/`, which prints the wall time of the run.
- Unit and direction: seconds; lower is better.
- Bound: under 120 s. `ux/playwright.config.ts` ends the run at 120 s (`globalTimeout`). With
  `UX_ROUND` set the limit of that file is 900 s; the workflow does not set it.
- Baseline: 48.0 s. Docs run.

### `docs-ux-step/run-time`

- For a reader: none directly; it is part of the time a change of the site waits for its
  verdict.
- Command: the step of `.github/workflows/docs-ux.yml` that runs
  `npx playwright test --workers=2`.
- Unit and direction: seconds; lower is better.
- Bound: under 120 s. The step has `timeout-minutes: 2`.
- Baseline: 49 s. Docs run.

### `docs-ux-workflow/run-time`

- For a reader: none directly; it is the time a change of the site waits for its verdict once a
  runner has it.
- Command: the workflow `.github/workflows/docs-ux.yml`, job `Docs UX gates`.
- Unit and direction: seconds; lower is better.
- Bound: under 1200 s. The job has `timeout-minutes: 20`.
- Baseline: 133 s. Docs run.

## Local speech-to-text adapters

Six values are not from a CI run. Each is the result of an ignored test that a person runs on
the reference machine and copies into the capability record of the adapter. The record names the
machine. The test repeats the measurement three times and prints the median. There is no
evidence record for these six: see [not-measured.md](not-measured.md).

The two commands, each with the release profile:

- sherpa-onnx: `INFERENCE_SHERPA_ONNX_STT_DIR=<dir> cargo test --release -p
  inference-sherpa-onnx --all-features --test stt_resources -- --ignored --nocapture`. It needs
  the pinned checkpoint unpacked in that directory.
- whisper.cpp: `INFERENCE_WHISPERCPP_STT_MODEL=<ggml-base.en.bin>
  INFERENCE_WHISPERCPP_STT_CLIP=<0.wav> cargo test --release -p inference-whispercpp --test
  stt_resources -- --ignored --nocapture`. It needs the pinned model file and the speech clip.

The conditions of the two plans are the ones the capability records state: the reference
machine, execution on the CPU, one onnxruntime thread for sherpa-onnx, and whisper.cpp 1.8.3 on
4 threads.

The real-time factor has a second measure per adapter, `<adapter>-stt-record/real-time-factor`.
It is the value as committed, and every `task test-run` holds its bound.

### `sherpa-onnx-stt-resources/real-time-factor`

- For a user: recognition on a CPU keeps up with speech.
- Unit and direction: thousandths of real time; lower is better.
- Bound: under 1000. The ignored test asserts it when it is run. Between two runs of it, the
  bound is held on the committed value: `sherpa-onnx-stt-record/real-time-factor`.
- Baseline: 131. `adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json`.

### `sherpa-onnx-stt-record/real-time-factor`

- For a user: the real-time factor the repository states for the adapter is one at which
  recognition keeps up with speech.
- Command: `task test-run`, the test `the_resource_profile_is_a_measured_cpu_profile` of
  `adapters/inference-sherpa-onnx/tests/stt_record.rs`. It reads the committed record and
  measures nothing.
- Unit and direction: thousandths of real time; lower is better.
- Bound: under 1000 and at least 1. The test fails otherwise, in every `task test-run`.
- Baseline: 131. Check run, which read
  `adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json`.

### `sherpa-onnx-stt-resources/load-time`

- For a user: how long the first call waits for the model.
- Unit and direction: milliseconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 4937 ms. `adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json`.

### `sherpa-onnx-stt-resources/peak-memory`

- For a user: how much memory the adapter needs.
- Unit and direction: MiB; lower is better.
- Bound: recorded, no bound.
- Baseline: 363 MiB. `adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json`.

### `whispercpp-stt-resources/real-time-factor`

- For a user: recognition on a CPU keeps up with speech.
- Unit and direction: thousandths of real time; lower is better.
- Bound: under 1000. The ignored test asserts it when it is run. Between two runs of it, the
  bound is held on the committed value: `whispercpp-stt-record/real-time-factor`.
- Baseline: 202. `adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json`.

### `whispercpp-stt-record/real-time-factor`

- For a user: the real-time factor the repository states for the adapter is one at which
  recognition keeps up with speech.
- Command: `task test-run`, the test `the_resource_profile_is_a_measured_cpu_profile` of
  `adapters/inference-whispercpp/tests/stt_record.rs`. It reads the committed record and
  measures nothing.
- Unit and direction: thousandths of real time; lower is better.
- Bound: under 1000 and at least 1. The test fails otherwise, in every `task test-run`.
- Baseline: 202. Check run, which read
  `adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json`.

### `whispercpp-stt-resources/load-time`

- For a user: how long the first call waits for the model.
- Unit and direction: milliseconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 630 ms. `adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json`.

### `whispercpp-stt-resources/peak-memory`

- For a user: how much memory the adapter needs.
- Unit and direction: MiB; lower is better.
- Bound: recorded, no bound.
- Baseline: 337 MiB. `adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json`.

## Adapters

The measures of each adapter crate are on [adapters.md](adapters.md), one section per crate:
tests run and passed, the slowest test, the time bounds its tests hold, the records its tests
hold, and the commands that print a latency. The latency measures are defined and have no
baseline. The measures of the two local speech-to-text adapters stay on this page, in the
section above.

## A release

A release is a version tag on `main`. The commit the tag points at holds the changelog entry of
the version, the version in `Cargo.toml`, and that version as the `adapter_revision` of every
record under `adapters/`. `AGENTS.md` has the rule.

No release has a gate of its own in the YAML. A gate of the formats cannot say that it judges
the commit a tag points at, so a gate `release` is left out: see
[not-measured.md](not-measured.md). What a release has to hold is what every commit on `main`
has to hold:

- everything the gate `check` requires, and, for the paths they name, the gates `slow` and
  `docs-ux`;
- the records of each adapter fit the code and carry the version of the workspace: the eight
  `<adapter>-records/held` measures on [adapters.md](adapters.md), the tests the sections of
  the two local speech-to-text adapters name there, and `generated-docs/current`, whose command
  refuses a record of another version;
- the statuses of the documentation agree with the changelog: the measure below.

The gate `check` requires the last two already. Of the workflows, only
`.github/workflows/shared-gates.yml` runs for a tag. Every other check ran on the commit before
it was tagged.

### `release-statuses/held`

- For a user: a page that says "Since" a version names the release that shipped the thing, and
  a page that says "On main" names something no release has yet.
- Command: `task test-run`.
- Unit and direction: none; holds, once per test.
- Bound: must hold for every test. `task test-run` fails otherwise.
- Baseline: holds for 4 of 4 tests. Check run.
- Bindings against the changelog: the test
  `every_binding_is_since_the_release_whose_changelog_section_introduced_it` of
  `crates/inference-docs/src/adapters.rs` holds that each binding of the adapters page names
  the release whose section of `CHANGELOG.md` first has its introducing phrase.
- On main: the test `a_binding_is_on_main_only_while_its_entry_is_unreleased` of
  `crates/inference-docs/src/adapters.rs` holds, on a changelog of its own, that the rule
  refuses a binding marked for the next release whose phrase is in a released section.
- Status items: the test
  `every_hosted_binding_is_a_shipped_item_since_the_release_of_its_adapter_row` of
  `crates/inference-docs/src/status.rs` holds that each hosted binding since a release has a
  shipped item of the status page that names that release.
- Next items: the test
  `a_hosted_binding_on_main_is_a_next_item_and_one_since_a_release_a_shipped_item` of
  `crates/inference-docs/src/status.rs` holds the same rule on items of its own, in both
  directions.

## What Ratchet's first working release could judge

That release does not gate sampled measurements and does not read conditions such as the runner,
the tool versions or the host load. It has no time windows, presets, exceptions or trade-offs.

It could judge the requirements on exact measurements:

- `formatted`, `no-lint-warnings`, `no-doc-warnings`, `generated-docs-current`,
  `planning-store-valid`, `specification-valid`, `native-archive-checksum`,
  `source-scan-passes`;
- `conformance-keeps-passing`;
- the twelve requirements of the documentation site, `docs-...-not-worse`;
- the four on the committed capability records: `sherpa-onnx-stt-record-keeps-up`,
  `sherpa-onnx-stt-record-is-measured`, `whispercpp-stt-record-keeps-up`,
  `whispercpp-stt-record-is-measured`;
- the nine on the time bounds of the adapters, `<adapter>-time-bounds-held`, and the eight on
  their records, `<adapter>-records-held`;
- `release-statuses-held`.

It would have nothing to judge for the latency measures of [adapters.md](adapters.md): they are
sampled, and no requirement names them.

It would only record the requirements on sampled measurements, every one of them a duration or
a ratio of durations:

- `test-budget`, `slow-test-budget`, `test-step-budget`, `check-job-within-timeout`,
  `slow-job-within-timeout`;
- `docs-test-budget`, `docs-run-budget`, `docs-step-within-limit`, `docs-job-within-timeout`;
- `sherpa-onnx-stt-keeps-up`, `whispercpp-stt-keeps-up`.

The bounds of those eleven stay enforced where they are today: in `.config/nextest.toml`,
`Taskfile.yml`, `ux/playwright.config.ts`, the three workflows and the two ignored tests.
