# The quality of this repository

This directory says what counts as the quality of `inference`, what each value is today, and
where the value comes from. What has no command that produces it is in
[not-measured.md](not-measured.md).

The documents are written in the formats of Ratchet (`beyond10x/ratchet`), against its
specification at commit `f139eccc17a548444f0f4e1dc06520120c28739f`.

- No Ratchet command works at that commit. Nothing here is run by Ratchet.
- Every field name is a proposal of that specification. A release of Ratchet can change it.
- The documents add no gate and change none. They describe what `task check`, `task test-slow`
  and the workflows under `.github/workflows/` already enforce.

## The files

| File | Format | Holds |
| --- | --- | --- |
| `../ratchet.toml` | local config | the specification files and the assessor directory |
| `gate.ratchet.yaml` | `ratchet/1` | the gate, the tests, the specification, turnaround |
| `docs-site.ratchet.yaml` | `ratchet/1` | the documentation site |
| `adapters.ratchet.yaml` | `ratchet/1` | the resource profile of the local speech-to-text adapters |
| `assessors/<name>@<version>.assessor.yaml` | `ratchet-assessor/1` | what each assessor reports |
| `records/<assessment>.evidence.json` | `ratchet-evidence/1` | one baseline per assessment, read from a CI run |

No assessor has a program. Each declaration is without `command`: it names the outputs, units
and scopes that a program would report. The records were written by hand from the CI runs named
below.

A test keeps the documents true: `crates/inference-docs/tests/quality/mod.rs`, part of
`task test-run`. It fails when a file stops parsing, when a requirement, an assessment or a
record names something that is not declared, when a unit differs from the assessor's, when a
`task <name>` command names a task `Taskfile.yml` does not have, and when a measure below is
missing from this page. It is not `ratchet validate`.

## The runs the baselines were read from

All ran on 2026-11-24. The first three ran on `main` at
`59b8f4a858ab9c4eef94bc21de8e7bb85f90ddad`.

| Name here | Workflow | Run |
| --- | --- | --- |
| check run | `check` | [45474241015](https://github.com/beyond10x/inference/actions/runs/45474241015) |
| slow run | `slow` | [45474240955](https://github.com/beyond10x/inference/actions/runs/45474240955) |
| gates run | `Shared source gates` | [45474241551](https://github.com/beyond10x/inference/actions/runs/45474241551) |
| docs run | `Docs UX` | [45467764733](https://github.com/beyond10x/inference/actions/runs/45467764733) |

`Docs UX` does not run on `main`. The docs run is of the pull request head
`adf479ee2707962b93a2c8ce6509bb53775024bb`, whose tree is the tree of `59b8f4a8`.

## How to read an entry

The heading is the measure: `<assessment>/<measurement>`, as the YAML names it. "Holds" means
the measure is an assertion: it has no unit, and its direction is that it must hold. "Recorded,
no bound" means nothing fails when the value gets worse.

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
- Baseline: the 472 ids named in `crates/inference-testkit/tests/conform.rs` passed, and
  `ess/SKIPPED.md` lists no skipped id. The test passed in the check run. The total the suite
  executed is not in the run: see [not-measured.md](not-measured.md). There is no record for
  this assessment.

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

### `slow-lane/slowest-test`

- For a user: a wrong use of a macro of the library gives the compile error the tests pin.
- Command: `task test-slow`.
- Unit and direction: seconds; lower is better.
- Bound: under 600 s. The `slow` profile of `.config/nextest.toml` ends a test after ten periods
  of 60 s.
- Baseline: 26.703 s, with 1 test run and 1 passed. Slow run.

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

## Documentation site

The twelve measures of `docs-ux` share what follows.

- Command: `npx playwright test --workers=2` under `ux/`, then `node tools/ratchet.ts`, which
  compares with `ux/baseline.json`. No task runs them; the workflow
  `.github/workflows/docs-ux.yml` does, on a pull request that touches `website/`, `docs/` or
  `ux/`.
- Direction: lower is better.
- Bound: no value may be worse than its value in `ux/baseline.json`, and a better value must be
  written to that file in the same change. `node tools/ratchet.ts` fails otherwise.
- Baseline: the values of `ux/baseline.json`. The docs run measured the same: 45 tests passed,
  80 values, 0 problems.
- A view is a scenario at a viewport, written `<scenario>/<viewport>`. The wide viewport is
  1440x900, the narrow one 390x844.

The first five are measured over every page and theme of the built site, scenario `sitemap`.

### `docs-ux/horizontal-scroll`

- For a reader: no page scrolls sideways.
- Unit: pages. Baseline: 0 at both viewports.

### `docs-ux/overflowing-elements`

- For a reader: nothing is cut off at the edge of the screen.
- Unit: elements. Baseline: 0 at both viewports.

### `docs-ux/always-visible-misses`

- For a reader: what must always be on screen is on screen.
- Unit: misses. Baseline: 0 at both viewports.

### `docs-ux/accessibility-violations`

- For a reader: a screen reader meets no serious or critical violation.
- Unit: nodes. Baseline: 0 at both viewports.

### `docs-ux/keyboard-unreachable`

- For a reader: everything that can be clicked can be reached with the keyboard.
- Unit: elements. Baseline: 0 at both viewports.

The other seven are measured for five tasks a reader has: find the dependency lines, the error
kinds, the limitations, the provider comparison, the variable of the realtime model. In the
tables the scenario is written without its prefix `find-`.

### `docs-ux/completion-misses`

- For a reader: the task ends on the page that answers it.
- Unit: misses. Baseline: 0 in all ten views.

### `docs-ux/steers`

- For a reader: the reader never has to go back a page.
- Unit: steers. Baseline: 0 in all ten views.

### `docs-ux/actions`

- For a reader: how many clicks the task takes, with opening the menu and going back.
- Unit: actions.

| Scenario | Wide | Narrow |
| --- | --- | --- |
| dependency-lines | 1 | 2 |
| error-kinds | 2 | 4 |
| limitations | 2 | 4 |
| provider-comparison | 1 | 2 |
| realtime-model-variable | 1 | 2 |

### `docs-ux/looks`

- For a reader: how often the next link is off screen and has to be scrolled to.
- Unit: looks.

| Scenario | Wide | Narrow |
| --- | --- | --- |
| dependency-lines | 0 | 0 |
| error-kinds | 1 | 1 |
| limitations | 1 | 1 |
| provider-comparison | 0 | 1 |
| realtime-model-variable | 1 | 1 |

### `docs-ux/steps-to-act`

- For a reader: how many links the reader follows to the page that answers the task.
- Unit: steps. A value within 5 percent of the baseline counts as equal.

| Scenario | Wide | Narrow |
| --- | --- | --- |
| dependency-lines | 1 | 1 |
| error-kinds | 2 | 2 |
| limitations | 2 | 2 |
| provider-comparison | 1 | 1 |
| realtime-model-variable | 1 | 1 |

### `docs-ux/glance-cost-median`

- For a reader: how hard the next link is to find, at the median step of the task.
- Unit: level. 1 is a link in the main navigation on screen, 2 a link elsewhere on screen, 3 a
  link off screen.

| Scenario | Wide | Narrow |
| --- | --- | --- |
| dependency-lines | 1 | 1 |
| error-kinds | 1 | 1 |
| limitations | 1 | 1 |
| provider-comparison | 1 | 1 |
| realtime-model-variable | 3 | 1 |

### `docs-ux/glance-cost-p95`

- For a reader: the same, at the hardest step of the task.
- Unit: level. 1 is a link in the main navigation on screen, 2 a link elsewhere on screen, 3 a
  link off screen.

| Scenario | Wide | Narrow |
| --- | --- | --- |
| dependency-lines | 1 | 1 |
| error-kinds | 3 | 3 |
| limitations | 3 | 3 |
| provider-comparison | 1 | 3 |
| realtime-model-variable | 3 | 3 |

## Local speech-to-text adapters

The values are not from a CI run. Each is the result of an ignored test that a person runs on
the reference machine and copies into the capability record of the adapter. The record names the
machine. The test repeats the measurement three times and prints the median. There is no
evidence record for these assessments: see [not-measured.md](not-measured.md).

The two commands, each with the release profile:

- sherpa-onnx: `INFERENCE_SHERPA_ONNX_STT_DIR=<dir> cargo test --release -p
  inference-sherpa-onnx --all-features --test stt_resources -- --ignored --nocapture`. It needs
  the pinned checkpoint unpacked in that directory.
- whisper.cpp: `INFERENCE_WHISPERCPP_STT_MODEL=<ggml-base.en.bin>
  INFERENCE_WHISPERCPP_STT_CLIP=<0.wav> cargo test --release -p inference-whispercpp --test
  stt_resources -- --ignored --nocapture`. It needs the pinned model file and the speech clip.

The baselines are read from `adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json` and
`adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json`.

### `sherpa-onnx-stt-resources/real-time-factor`

- For a user: recognition on a CPU keeps up with speech.
- Unit and direction: thousandths of real time; lower is better.
- Bound: under 1000. The ignored test asserts it when it is run.
- Baseline: 131.

### `sherpa-onnx-stt-resources/load-time`

- For a user: how long the first call waits for the model.
- Unit and direction: milliseconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 4937 ms.

### `sherpa-onnx-stt-resources/peak-memory`

- For a user: how much memory the adapter needs.
- Unit and direction: MiB; lower is better.
- Bound: recorded, no bound.
- Baseline: 363 MiB.

### `whispercpp-stt-resources/real-time-factor`

- For a user: recognition on a CPU keeps up with speech.
- Unit and direction: thousandths of real time; lower is better.
- Bound: under 1000. The ignored test asserts it when it is run.
- Baseline: 202.

### `whispercpp-stt-resources/load-time`

- For a user: how long the first call waits for the model.
- Unit and direction: milliseconds; lower is better.
- Bound: recorded, no bound.
- Baseline: 630 ms.

### `whispercpp-stt-resources/peak-memory`

- For a user: how much memory the adapter needs.
- Unit and direction: MiB; lower is better.
- Bound: recorded, no bound.
- Baseline: 337 MiB.

## What Ratchet's first working release could judge

That release does not gate sampled measurements and does not read conditions such as the runner,
the tool versions or the host load. It has no time windows, presets, exceptions or trade-offs.

It could judge the requirements on exact measurements:

- `formatted`, `no-lint-warnings`, `no-doc-warnings`, `generated-docs-current`,
  `planning-store-valid`, `specification-valid`, `native-archive-checksum`,
  `source-scan-passes`;
- `conformance-keeps-passing`;
- the twelve requirements of the documentation site, `docs-...-not-worse`.

It would only record the requirements on sampled measurements, every one of them a duration or
a ratio of durations:

- `test-budget`, `slow-test-budget`, `test-step-budget`, `check-job-within-timeout`;
- `sherpa-onnx-stt-keeps-up`, `whispercpp-stt-keeps-up`.

The bounds of those six stay enforced where they are today: in `.config/nextest.toml`,
`Taskfile.yml`, `.github/workflows/check.yml` and the two ignored tests.
