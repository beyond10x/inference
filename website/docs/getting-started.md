---
title: Getting started
sidebar_position: 2
description: Run the first example with no model and no network, build and check the workspace, and add inference to your own project.
lede: One contract and a mock recogniser print two committed spans; the rest of the workspace builds after one pinned native archive is fetched.
source: examples/stt-only, Taskfile.yml, .cargo/config.toml, Cargo.toml, CHANGELOG.md 0.7.0
---

# Getting started

## Requirements

| For | You need |
|---|---|
| building | Rust stable, 1.98 or later |
| `task check` | [go-task](https://taskfile.dev), and the `ess` and `aep` command-line tools |
| the sherpa-onnx adapter | Linux x86_64: `task sherpa-onnx-archive` fetches the pinned `linux-x64` native archive |
| the whisper.cpp adapter | `cmake`, a C++ compiler and `libclang`; whisper.cpp is compiled from source, CPU only |
| the voice examples | a microphone and an output device; use a headset, there is no echo cancellation |

## Run the first example

```sh
git clone https://github.com/beyond10x/inference
cd inference
cargo run -p stt-only --example stt_only
```

```text
committed: hello there
committed: how are you
```

The example feeds a fixture of audio frames to the mock recogniser through the STT contract and
prints each span it commits. It depends on `inference-stt` and on `inference-testkit` with its
`stt` feature. The testkit brings the vocabulary, contract and pipeline crates (`inference-core`,
`inference-audio`, `inference-text`, `inference-tools`, `inference-duplex`, `inference-pipeline`),
`serde_json` and `uuid`, but no `inference-runtime`, no async runtime, no model and no network
client. The test
[`examples/stt-only/tests/output.rs`](https://github.com/beyond10x/inference/blob/main/examples/stt-only/tests/output.rs)
holds those two lines.

## Build and check the workspace

```sh
task sherpa-onnx-archive
cargo build --workspace
task check
```

`task sherpa-onnx-archive` fetches the sherpa-onnx native library built without TTS, checks it
against its pinned sha256 and unpacks it under `target/sherpa-onnx`, where `.cargo/config.toml`
points `SHERPA_ONNX_LIB_DIR`. A build without it fails rather than downloading anything. What
`task check` runs, and what a passing run prints, is in [Run the checks](guides/run-the-checks.md).

## Use it in your project

A release is a Git tag of the repository; depend on it through Git. Take the contract you need and
the adapter that implements it, and switch on the adapter features you use:

```toml
[dependencies]
inference-stt = { git = "https://github.com/beyond10x/inference", tag = "0.7.0" }
inference-duplex = { git = "https://github.com/beyond10x/inference", tag = "0.7.0" }
inference-openai = { git = "https://github.com/beyond10x/inference", tag = "0.7.0", features = ["duplex", "keychain"] }
```

Every crate, its features and what it holds is listed in [Crates](reference/crates.md); which
feature turns on which adapter binding is in [Adapters](reference/adapters.md).

## Upgrading to 0.7.0

0.7.0 adds a usage record at the end of every recognition and synthesis, and that changes two
signatures:

| Before 0.7.0 | From 0.7.0 |
|---|---|
| `TranscriptEvent::span_id` returns a `SpanId` | returns `Option<SpanId>`: the `Usage` event that ends a recognition names no span |
| `SynthesisEvent::segment_id` returns a `SegmentId` | returns `Option<SegmentId>`: the `Usage` event that ends a synthesis names no segment |

After a recognition's usage record, any further move on it is refused with `RecognitionEnded`;
after a synthesis's, with `SynthesisEnded`. The record itself is refused while a span is still
uncommitted or a segment is not synthesized, unless the call failed. A caller that matched on
every `TranscriptEvent` or `SynthesisEvent` variant also has a new `Usage` variant to handle.

## Next

- [Talk to a hosted voice model](guides/talk-to-a-hosted-voice-model.md)
- [Run the voice cascade offline](guides/run-the-local-voice-cascade-offline.md)
- [Status](status.mdx): what has shipped and what is planned
