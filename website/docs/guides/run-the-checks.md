---
title: Run the checks
sidebar_position: 7
description: What task check runs, in which order, how to run the conformance suite on its own, and which tests are ignored without a model, a server or a key.
lede: One command validates the specification, the plan and the generated documentation, then formats, lints and tests the workspace, as CI does on every pull request.
source: Taskfile.yml, ess/, crates/inference-testkit (tests/conform.rs), crates/inference-docs
---

# Run the checks

:::note[Requirements]

Rust stable 1.98 or later, [go-task](https://taskfile.dev), and the `ess` and `aep` command-line
tools. `task check` fetches the pinned sherpa-onnx archive on its first run; whisper.cpp is compiled
from source, so `cmake`, a C++ compiler and `libclang` are needed too.

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
| tests | `cargo test --workspace --all-features` |

CI runs the same task on every pull request.

The steps as `task check` prints them, from a passing run in this tree:

```text
task: Task "sherpa-onnx-archive" is up to date
task: [check] ess specify validate --path ess
task: [check] aep plan artifact validate
task: [docs-check] cargo run -q -p inference-docs -- generate --check
task: [check] cargo fmt --all --check
task: [check] cargo clippy --workspace --all-targets --all-features -- -D warnings
task: [check] cargo test --workspace --all-features
```

In that run, every one of the 314 test targets ended `test result: ok`: 2177 tests passed and 50 ignored.

## The conformance suite alone

The specification's scenarios run against the testkit's target:

```sh
ess specify validate --path ess
cargo test -p inference-testkit --features run --test conform
```

With ess 0.76.0, the release CI installs, the validation prints the following. Long lines are
wrapped here, and their continuation lines are indented four spaces:

```text
warning[ESS-ENTITY-019]: `inference.session.HistoryEntry` stores `call_id`, typed
    `Optional<inference.tools.ToolCallId>`, which identifies `inference.tools.ToolCall`,
    and no relation declares that it names a `inference.tools.ToolCall`
  `ess-domain` warns of this as `implied_relation`; it is legal and compiles as written
  help: declare a `references` relation on `inference.session.HistoryEntry`:
    `{name: call_id, kind: references, target: inference.tools.ToolCall,
    cardinality: one, via: call_id}`
  --> domains/session.yaml:173:5
inference v1 — 11 file(s), 9 scenario(s), valid
```

The last lines of the test run:

```text
test the_suite_runs_against_the_testkit_target ... ok

test result: ok. 66 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.58s
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

## The generated pages

The crate list, the adapter matrix, the error list and the status data of this site are generated
from the workspace metadata, the adapter records, the specification and the test list. After a
change to any of them:

```sh
task docs-generate
```

`task docs-check` fails when a generated file is missing, stale or edited by hand, or when a page
names a dependency, tag or feature the workspace does not have.
