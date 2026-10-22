---
format: aep.planning-md/3
id: story:sdk-crate
kind: story
status: draft
title: The inference crate re-exports the workspace by feature and has one Error that names its fix
relations:
- decomposes: epic:sdk
- serves: vision:inference
scope:
- confidence: inferred
  path: crates/inference-macros/Cargo.toml
- confidence: inferred
  path: crates/inference-macros/src/lib.rs
- confidence: inferred
  path: crates/inference/Cargo.toml
- confidence: inferred
  path: crates/inference/src/agent.rs
- confidence: inferred
  path: crates/inference/src/chat.rs
- confidence: inferred
  path: crates/inference/src/devices.rs
- confidence: inferred
  path: crates/inference/src/error.rs
- confidence: inferred
  path: crates/inference/src/event.rs
- confidence: inferred
  path: crates/inference/src/graph.rs
- confidence: inferred
  path: crates/inference/src/lib.rs
- confidence: inferred
  path: crates/inference/src/macro_selftest.rs
- confidence: inferred
  path: crates/inference/src/oneshot.rs
- confidence: inferred
  path: crates/inference/src/payload.rs
- confidence: inferred
  path: crates/inference/src/prelude.rs
- confidence: inferred
  path: crates/inference/src/recipe.rs
- confidence: inferred
  path: crates/inference/src/registry/aws.rs
- confidence: inferred
  path: crates/inference/src/registry/llama_server.rs
- confidence: cited
  path: crates/inference/src/registry/mock.rs
- confidence: inferred
  path: crates/inference/src/registry/mod.rs
- confidence: inferred
  path: crates/inference/src/registry/openai.rs
- confidence: inferred
  path: crates/inference/src/registry/sherpa_onnx.rs
- confidence: inferred
  path: crates/inference/src/registry/whispercpp.rs
- confidence: inferred
  path: crates/inference/src/stack.rs
- confidence: inferred
  path: crates/inference/src/testing.rs
- confidence: inferred
  path: crates/inference/src/transcriber.rs
- confidence: inferred
  path: crates/inference/src/usage.rs
- confidence: inferred
  path: crates/inference/src/voice.rs
- confidence: inferred
  path: crates/inference/tests/agent.rs
- confidence: inferred
  path: crates/inference/tests/oneshot.rs
- confidence: inferred
  path: crates/inference/tests/recipes.rs
- confidence: inferred
  path: crates/inference/tests/testing.rs
- confidence: inferred
  path: crates/inference/tests/usage_cost.rs
revision: 19
---
## Context

`docs/design/sdk.md` §3 (L4) and §9: the `inference` crate, its feature-selected re-exports, its
prelude and its one error type. The crate the design reserves at `docs/design/inference.md:245`.
Targets T3 (no adapter crate without its feature) and T7. Serves O1.

This story also writes, once, every module, re-export, feature and dependency the later SDK stories
need, so that no later story edits `crates/inference/src/lib.rs`, `src/prelude.rs` or
`crates/inference/Cargo.toml` (decision D9 of `api-design:sdk`).

## Domain relations

No new noun.

## Planned scope

- `crates/inference/Cargo.toml` (new).
  - Features: `default = []` (no adapter, no device, no cost: an application names what it uses);
    `sherpa-onnx`, `whispercpp`, `llama-server`, `openai`, `openai-live`, `aws`;
    `audio-host` (`inference-audio-device` with `default-features = false` and `features =
    ["pipeline"]`: `AudioHost`, `FakeHost`, whose module `fake` is unconditional, and
    `PlaybackPayload`, behind `pipeline`; no cpal, `adapters/inference-audio-device/Cargo.toml:10-13`,
    `src/lib.rs`); `audio-device` (`["audio-host", "inference-audio-device/device"]`, adding cpal
    for `CpalHost`, which sits behind `device`); `cost` (`inference-facade/cost`); `testing`
    (`inference-testkit` with `run`, `stt`, `llm`, `tts`, `duplex`, and the feature `audio-host`,
    so `testing` gives `FakeHost` and implies no cpal).
  - Dependencies: every `crates/inference-*` contract crate, `inference-facade`,
    `inference-macros`, `tokio` (features `macros`, `rt`, `rt-multi-thread`, `signal`, `sync`, `time`:
    the multi-thread runtime `#[inference::main]` starts, and Ctrl-C for story:sdk-agent),
    `futures-core`, `futures-util` (`StreamExt`), `tokio-stream` (features `sync`: the
    `BroadcastStream` story:sdk-agent's `Events` wraps; 0.1, as `adapters/inference-aws/Cargo.toml:22`),
    `serde`, `serde_json`, `toml`, `schemars`, `hound`
    (the WAV reader of `transcribe(path)`, story:sdk-one-call); each adapter crate and
    `inference-testkit` optional behind its feature.
  - Dev-dependencies: `trybuild`, `tempfile`, `inference-testkit` (features `run`, `stt`, `llm`,
    `tts`, `duplex`), `inference-audio-device` (`default-features = false`, `features =
    ["pipeline"]`, so the tests build no cpal either).
  - `[[test]]` entries with `required-features`, as `crates/inference-facade/Cargo.toml:29-31` does
    for `cost`: `usage_cost` → `cost`; `agent` → `testing` (which implies `audio-host`; the agent tests bind the
    testkit mocks, story:sdk-agent); `oneshot` → `testing`;
    `testing` → `testing`; `recipes` → `testing`. `stack`, `graph` and `tool_macro` require no
    feature and are found by Cargo as usual, so `cargo test -p inference --no-default-features
    --test stack` runs (story:sdk-stack).
- One stub file for each declared `[[test]]`, so Cargo accepts the manifest:
  `crates/inference/tests/usage_cost.rs`, `agent.rs`, `oneshot.rs`, `testing.rs` and `recipes.rs`,
  each holding only `#![cfg(feature = "<its required feature>")]`. The owning stories fill them
  (story:sdk-usage-cost, story:sdk-agent, story:sdk-one-call, story:sdk-testing,
  story:sdk-recipes).
- `crates/inference/src/lib.rs` (new): `extern crate self as inference;`, so the macros' `::inference::`
  paths resolve inside this crate too; `mod macro_selftest;` over the file `src/macro_selftest.rs`, holding only its doc comment,
  which story:sdk-tool-macro fills with a unit test and a doc test of the macros; `core`, `text`, `audio`, `stt`, `llm`, `tts`, `duplex`,
  `pipeline`, `runtime`, `facade` re-exported as modules (so `inference::pipeline::{Catalog,
  Declaration, PipelineId}`, `inference::core::policy::ProfileId` and
  `inference::core::model::{AdapterId, Capability}`, the types story:sdk-recipes'
  `Recipe::declare_into` takes, R195), `audio_device`
  (`inference-audio-device`) behind `audio-host` so an example lists no adapter crate for its
  devices (story:sdk-examples, T3), and `pub mod tools` re-exporting
  `inference_tools` (`::inference::tools::{Tool, ToolExecutor, SideEffectClass, …}`, the paths the
  macro emits); `#[doc(hidden)] pub mod __private { pub use serde; pub use schemars; pub use
  serde_json; pub use tokio; pub fn exit(..) }`; `pub use inference_macros::{main, tool}`; `pub mod` for each SDK module, with
  `pub use <module>::*` where the design puts items at the crate root, so an empty module
  re-exports nothing and a later story adds items without touching `lib.rs`.
- Stub modules, each an empty file with its doc comment: `payload.rs`, `graph.rs`, `stack.rs`,
  `registry/mod.rs`, `registry/sherpa_onnx.rs`, `registry/whispercpp.rs`, `registry/llama_server.rs`,
  `registry/openai.rs` (both `openai` and `openai-live`), `registry/aws.rs`, `registry/mock.rs`
  (behind `testing`), `recipe.rs`, `agent.rs`, `event.rs`, `devices.rs`, `oneshot.rs`, `chat.rs`,
  `transcriber.rs`, `voice.rs`, `usage.rs`, `testing.rs` (behind `testing`); each adapter registry
  file behind its feature, `devices.rs` behind `audio-host`. In `oneshot.rs`, `speak_with` and
  `talk_with` are declared behind `audio-host` (they take an `AudioHost`), and `speak` and `talk`
  behind `audio-device` (they open the default host, `CpalHost`); `transcribe`, `chat`,
  `transcribe_with` and `chat_with` are not (decision D18 of `api-design:sdk`).
- `crates/inference/src/error.rs` (new): `Error`; `pub enum ErrorKind`, `#[non_exhaustive]`, with
  one value per `Error` variant and named as it (`AdapterNotCompiled`, `RoleUnbound`,
  `ProfileDenies`, `StackFile`, `Device`, `Provider`, `NoPrices`, `Closed`, `Layer`), deriving
  `Clone, Copy, Debug, PartialEq, Eq`, and `Error::kind(&self) -> ErrorKind` (R199). Both sit at
  the crate root through the `pub use error::*` that carries `Error`, so `inference::ErrorKind`.
  `Error::Layer { message: String, source: Option<Box<dyn std::error::Error + Send + Sync>> }`:
  its `Display` is `message`, and `source()` returns `source`.
  `Error::ProfileDenies { role: String, adapter: String, locality:
  inference_core::model::Locality, profile: inference_core::policy::ProfileKind, source:
  inference_pipeline::LocalityDenied }` (R203, R212). `role` is a `String`, the role's lowercase
  name (`stt`, `llm`, `tts`, `speech`), because `Role` is story:sdk-stack's; story:sdk-recipes'
  instantiation fills it from the denied node's role. Its `Display` names the role, the adapter
  and the locality it runs at, then the profile that does not admit it. The
  locality and the profile are written as their snake_case serde names (`external_provider`,
  `offline_strict`), matched in `error.rs`, since `Locality` has no `Display`
  (`crates/inference-core/src/model.rs:104-111`). Then comes the fix, the exact text
  `profile = "hybrid_opt_in"`, and every source that sets it: the
  top-level key of a stack file, `INFERENCE_PROFILE=hybrid_opt_in` and
  `Stack::profile(ProfileKind::HybridOptIn)` (story:sdk-stack, R204); `source()` returns the
  `LocalityDenied` (`crates/inference-pipeline/src/lib.rs:476-481`). `Result`, and
  `report(result, &mut impl Write) ->
  ExitCode`, which writes nothing and returns `SUCCESS` for `Ok`, and for `Err(e)` writes
  `error: {e}` then one `caused by: {source}` line per `source()` in the chain, all through
  `Display`, and returns `FAILURE`. `inference::__private::exit(result)` is `report` over stderr;
  the `fn main` that `#[inference::main]` emits returns it (story:sdk-tool-macro).
- `crates/inference/src/prelude.rs` (new). The prelude re-exports by glob the SDK
  modules whose items the design puts at the crate root (`agent`, `event`, `stack`, `recipe`,
  `chat`, `transcriber`, `voice`, `usage`), plus `Error`, `ErrorKind` (R206), `Result` and, behind `audio-host`,
  `inference_audio_device::AudioHost`, so the §3.2 program, whose `main` takes
  `host: impl AudioHost, stack: Stack`, needs only `use inference::prelude::*;`.
- `crates/inference-macros/` (new), owned by this story alone: `Cargo.toml` (`proc-macro = true`,
  `syn` with the feature `full`, `quote`, `proc-macro2`) and `src/lib.rs` with a `tool` and a `main`
  attribute, each emitting `compile_error!` until story:sdk-tool-macro, which then writes `src/lib.rs` and never
  `Cargo.toml`.
- The root `Cargo.toml` needs no edit: `members = ["crates/*", "examples/*",
  "adapters/*"]` (`Cargo.toml:3`).

## Acceptance

- `inference::Error` is `#[non_exhaustive]` with the variants of `docs/design/sdk.md` §9, `kind()`
  and `source()`. A snapshot test holds every variant's `Display`. Six of them name their fix, each
  asserted by substring in a test:
  - `AdapterNotCompiled`: the line `features = ["<feature>"]`;
  - `RoleUnbound`: `INFERENCE_<ROLE>_ADAPTER` and the `[<role>]` table;
  - `ProfileDenies`: the text `profile = "hybrid_opt_in"`, `INFERENCE_PROFILE` and
    `Stack::profile`, and the role, adapter and locality it was built with, the three before
    the fix text (R203, R212);
  - `StackFile`: the file path and the key;
  - `Device`: the selector and the device names `AudioHost::devices`
    (`adapters/inference-audio-device/src/host.rs:156`) returned;
  - `NoPrices`: the key `prices`.
  `Provider`, `Closed` and `Layer` have the snapshot only.
- `kind()` (R199): a test builds one value of each of the nine variants and asserts that its
  `kind()` is the `ErrorKind` value of the same name; for `Error::Layer` with a `source` it also
  asserts that `source()` returns that error, and with `None` that `source()` is `None`.
- `cargo check -p inference --no-default-features` and `cargo check -p inference --all-features`
  exit 0, and so does `cargo check -p inference-macros`.
- A doc test compiles `use inference::prelude::*;` and reaches one type from each re-exported crate,
  and names `ErrorKind::Layer` with no other import (R206).
- `cargo tree -p inference -e normal --no-default-features` lists no crate under `adapters/`.
- A unit test in `error.rs` calls `report` with a `Vec<u8>` writer: for
  `Err(Error::AdapterNotCompiled { .. })` naming `whispercpp` the output starts with `error: ` and
  contains `features = ["whispercpp"]`, and the code is `ExitCode::FAILURE`; for `Ok(())` the
  output is empty and the code `ExitCode::SUCCESS`.
- `rg -n "^extern crate self as inference;" crates/inference/src/lib.rs` matches; the unit test
  that uses `#[inference::tool]` inside this crate is story:sdk-tool-macro's (`src/macro_selftest.rs`),
  because the attribute is a `compile_error!` stub until then.
- Cargo accepts the manifest with every `[[test]]` file present: `cargo test -p inference
  --all-features --no-run` exits 0.
- `cargo check -p inference --features testing` enables `audio-host` and not `audio-device`:
  `cargo tree -p inference -e features --features testing` lists `inference feature "audio-host"`
  and no `inference feature "audio-device"`, and `cargo tree -p inference -e normal --features
  testing` lists no `cpal`; with `--features audio-device` it lists `cpal`.
- `cargo check -p inference --no-default-features --features audio-host` and
  `cargo check -p inference --no-default-features --features audio-device` exit 0.

## Out of scope

The content of the stub modules (their own stories); publishing.
