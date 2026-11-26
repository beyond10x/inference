---
title: Run the checks
sidebar_position: 7
description: What task check runs, in which order, how to run the conformance suite on its own, which tests are ignored without a model, a server or a key, and the quality documents a test of task test-run keeps true.
lede: One command validates the specification, the plan and the generated documentation, then formats, lints and tests the workspace, as CI does on every pull request.
source: Taskfile.yml, ess/, crates/inference-testkit (tests/conform.rs), crates/inference-docs (tests/quality/), quality/, ratchet.toml, .github/workflows/check.yml
---

# Run the checks

:::note[Requirements]

The rustc `rust-toolchain.toml` pins (rustup installs it on first use),
[go-task](https://taskfile.dev), [cargo-nextest](https://nexte.st) at the version
`.config/nextest.toml` requires, and the `ess` and `aep` command-line tools. `task check` fetches
the pinned sherpa-onnx archive on its first run; whisper.cpp is compiled from source, so `cmake`, a
C++ compiler and `libclang` are needed too.

:::

```sh
task check
```

It runs, in order, and stops at the first failure:

| Step | Command |
|---|---|
| the verified sherpa-onnx native library | `task sherpa-onnx-archive` |
| the specification | `ess specify validate --path ess` |
| the planning store | `aep plan artifact validate` |
| the generated documentation | `task docs-check`, which is `cargo run -q -p inference-docs -- generate --check` |
| format | `cargo fmt --all --check` |
| lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| the API documentation | `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` |
| the build of the tests | `task test-build`, which runs no test |
| tests | `task test-run`, which runs `task test-run-parts` under `timeout`: `cargo nextest run --workspace --all-features`, then the stack test without default features and the doc tests |
| the quality records | `task quality-records`, which writes the record of each assessment of `quality/` that reads the report of the tests to `target/quality-records/` |

The first seven steps are `task check-source`. Under `.config/nextest.toml` a test that runs over 60
seconds is killed and fails, and the test step prints its ten slowest tests, also when one failed,
or `no test ran` when the run left no report. The test step itself is ended after 120 seconds and
fails then: `task test-run` exits 201, as it does when a test fails, and prints the line
`task test-run was ended at its bound of 120 s`, which a run that only has a failing test does not
print.

The bound is two signals. After 120 seconds `timeout` sends the step a first signal: the tests are
ended, and the step prints the list of its slowest tests and the line. A part that is still running
10 seconds later is killed by a second signal: the step then ends 130 seconds after it started, and
prints the line but not the list of its slowest tests. The `timeout` of GNU coreutils kills its
whole process group at the second signal. One that kills only its own child there, as the `timeout`
of uutils coreutils on the image of the CI runner does, can leave a part that ignored the first
signal running until the job ends.

CI runs the same four tasks, `task check-source`, `task test-build`, `task test-run` and
`task quality-records`, as separate steps of the required check `inference / task check`, and on
a push to `main` keeps the records as the artifact `quality-records-<commit>`
([The quality of this repository](https://github.com/beyond10x/inference/blob/main/quality/README.md#the-records-of-a-check-run)
says how to download them). A pull request whose changed files
are all under `.engineering/` or are `*.md` files outside `website/`, `ess/` and `quality/`, other
than `CHANGELOG.md`, only validates the planning store, and the check passes without a build. A
change to a file under `quality/`, a `*.md` file too, runs the full check: a test of
`task test-run` reads those files, as [The quality documents](#the-quality-documents) says.

The steps as `task --dry check` lists them in this tree, the script of `quality-records` cut after
its first line (the whole script is in `Taskfile.yml`):

```text
task: Task "sherpa-onnx-archive" is up to date
task: [check-source] ess specify validate --path ess
task: [check-source] aep plan artifact validate
task: [docs-check] cargo run -q -p inference-docs -- generate --check
task: [check-source] cargo fmt --all --check
task: [check-source] cargo clippy --workspace --all-targets --all-features -- -D warnings
task: [check-source] RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
task: [test-build] cargo nextest run --workspace --all-features --no-run
task: [test-build] cargo nextest run -p inference --no-default-features --test stack --no-run
task: [test-run] timeout --kill-after=10s 120s task test-run-parts || { status=$?; if [ "$status" -eq 124 ] || [ "$status" -eq 137 ]; then echo "task test-run was ended at its bound of 120 s"; fi; exit "$status"; }
task: [quality-records] rm -rf target/quality-records target/quality-records.list
task: [quality-records] mkdir -p target/quality-records
task: [quality-records] cargo run -q -p inference-docs -- junit-assessments > target/quality-records.list
task: [quality-records] report=target/nextest/default/junit.xml
```

And the parts of the test step, as `task --dry test-run-parts` lists them:

```text
task: [test-workspace] rm -f target/nextest/default/junit.xml
task: [test-workspace] cargo nextest run --workspace --all-features
task: [test-slowest] echo "the ten slowest tests, in seconds:"
task: [test-slowest] if [ -f target/nextest/default/junit.xml ]; then grep '<testcase ' target/nextest/default/junit.xml | sed -E 's/.* name="([^"]*)" classname="([^"]*)".* time="([^"]*)".*/\3 \2 \1/' | sort -rn | head -n 10; else echo "no test ran"; fi
task: [test-run-parts] cargo nextest run -p inference --no-default-features --test stack --profile stack
task: [test-run-parts] cargo test --workspace --all-features --doc
```

## The slow lane

The `trybuild` cases of the `inference` crate, what its macros and its agent builder accept and
refuse, run in one `trybuild` run, which builds the crate and its dependencies once more on its own:

```sh
task test-slow
```

It runs `cargo nextest run -p inference --features audio-device --profile slow`, and go-task skips
it while none of its sources changed: the macro crate, the sources of `inference` and
`inference-tools`, the cases, `Cargo.lock`, `rust-toolchain.toml` and `.config/nextest.toml`. The
workflow `slow` runs it in CI on a change to those paths. The `.stderr` snapshots are rustc's
messages: bless them (`TRYBUILD=overwrite`) only with the rustc `rust-toolchain.toml` pins.

## The conformance suite alone

The specification's scenarios run against the testkit's target:

```sh
ess specify validate --path ess
cargo test -p inference-testkit --features run --test conform
```

With ess 0.98.0, the release CI installs, the validation prints the following. Long lines are
wrapped here, and their continuation lines are indented four spaces:

```text
warning[ESS-ENTITY-019]: `inference.session.HistoryEntry` stores `call_id`, typed
    `Optional<inference.tools.ToolCallId>`, which identifies `inference.tools.ToolCall`,
    and no relation declares that it names a `inference.tools.ToolCall`
  `ess-domain` warns of this as `implied_relation`; it is legal and compiles as written
  help: declare a `references` relation on `inference.session.HistoryEntry`:
    `{name: call_id, kind: references, target: inference.tools.ToolCall,
    cardinality: one, via: call_id}`
  --> domains/session.yaml:174:5
inference v1 — 11 file(s), 10 scenario(s), valid
```

The last lines of the test run:

```text
test the_suite_runs_against_the_testkit_target ... ok

test result: ok. 66 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.28s
```

## Tests that need a model, a server or a key

Tests download no weights and start no model server. A test that needs one reads its path, URL or
key from an environment variable and is ignored without it, so `task check` passes on a machine
with none of them. To run them:

| Needs | Variables | Run |
|---|---|---|
| the sherpa-onnx checkpoint | `INFERENCE_SHERPA_ONNX_STT_DIR` | `cargo test -p inference-sherpa-onnx --all-features -- --ignored` |
| the whisper.cpp checkpoint and a clip | `INFERENCE_WHISPERCPP_STT_MODEL`, `INFERENCE_WHISPERCPP_STT_CLIP` | `cargo test -p inference-whispercpp -- --ignored` |
| a loopback llama-server | `INFERENCE_LLAMA_SERVER_URL`, `INFERENCE_LLAMA_SERVER_MODEL` | `cargo test -p inference-llama-server --all-features -- --ignored` |
| all of the above | as above, plus `INFERENCE_CASCADE_STT` | `cargo test -p local-voice-cascade --all-features -- --ignored`, once per recogniser |
| an OpenAI key (billed) | the keychain secret `openai-api-key`, or `OPENAI_API_KEY` with `INFERENCE_OPENAI_KEY_SOURCE=env` | `cargo test -p inference-openai --features live,gpt-live -- --ignored` |
| AWS access to Nova Sonic (billed) | the AWS default chain, `AWS_REGION` | `cargo test -p inference-aws --features live -- --ignored` |

Every variable is in [Environment](../reference/environment.md).

## The quality documents

The directory [`quality/`](https://github.com/beyond10x/inference/tree/main/quality) and the file
[`ratchet.toml`](https://github.com/beyond10x/inference/blob/main/ratchet.toml) say what counts as
the quality of this repository, what each value is today and where it comes from:

| File | Holds |
|---|---|
| `quality/README.md` | the measures of the gate, the tests, the specification, the turnaround, the documentation site, the local speech-to-text adapters and a release, each with its command, unit, direction, bound and baseline, and the CI runs the baselines were read from |
| `quality/adapters.md` | one section per crate under `adapters/`: the tests run and passed and the slowest test, the time bounds and the records its tests hold, and the commands that print a latency, which have no baseline |
| `quality/not-measured.md` | what is not measured, and what would produce the value |
| `ratchet.toml`, `quality/*.ratchet.yaml`, `quality/assessors/`, `quality/records/` | the same measures in the formats of Ratchet (`beyond10x/ratchet`), what each assessor reports, and one record per assessment whose baseline was read from a CI run |

The formats are those of the Ratchet specification at the commit `quality/README.md` names. No
Ratchet command works at that commit, so Ratchet runs none of the files. They are a gate all the
same: a step of `task check` and of the required check `inference / task check` reads them, and
`task quality-records` fails when a plan file cannot be read, when the plan files list no
assessment it writes a record for, when the declaration of an assessor is missing or cannot be
read, when `inference-docs assess` refuses its call for an assessment or cannot write the
record, or when a record is refused because the report of the tests lacks what its assessment
reads. Beyond that step they describe what `task check`, `task test-slow` and the workflows
under `.github/workflows/` enforce.

The tests of `crates/inference-docs/tests/quality/` run in `task test-run`. They fail when the
YAML files and records stop agreeing with each other, with `Taskfile.yml`, with the tree, or with
the pages that describe them, `quality/README.md` and `quality/adapters.md`: a measure without its
entry, a baseline that differs from its record, a command that calls a task `Taskfile.yml` does
not have, a test or an adapter directory a page names that does not exist. `quality/README.md` lists every rule under "The test
that keeps the documents true".

## The generated pages

The crate list, the adapter matrix, the error list and the status data of this site are generated
from the workspace metadata, the adapter records and sources, the specification and the test
list. After a change to any of them:

```sh
task docs-generate
```

`task docs-check` fails when a generated file is missing, stale or edited by hand, or when a page
names a dependency, tag or feature the workspace does not have.
