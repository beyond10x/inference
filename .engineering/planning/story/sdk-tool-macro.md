---
format: aep.planning-md/3
id: story:sdk-tool-macro
kind: story
status: draft
title: The inference::tool attribute turns an async fn into a tool definition and executor
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-crate
- depends_on: story:sdk-stack
scope:
- confidence: inferred
  path: crates/inference-macros/src/lib.rs
- confidence: inferred
  path: crates/inference/src/macro_selftest.rs
- confidence: inferred
  path: crates/inference/tests/tool_macro.rs
- confidence: inferred
  path: crates/inference/tests/ui
revision: 11
---
## Context

`docs/design/sdk.md` §5: `#[inference::tool]` on an `async fn` produces a tool definition and its
executor. Today an application writes the JSON schema and the dispatch by hand
(`examples/realtime-voice/src/lib.rs:83-104`, `time_tool` and `local_time`). Serves O3: effects stay
under tool authority.

`docs/design/sdk.md` §3.2: `#[inference::main]` turns `async fn main(host, stack)` into the
program's entry point and the `run` seam its tests call, so T1 counts the program as written and
T2/T4 drive the same body with a fake host and a scripted stack.

Ordered after story:sdk-stack: the `main` expansion calls `Stack::from_env()`, and its `run` test
binds the `mock` adapter through `Stack::bind` (both that story's).

## Domain relations

No new noun: the macro produces an `inference.tools.Tool` (`ess/domains/tools.yaml`).

## Planned scope

- `crates/inference-macros/src/lib.rs` (the stub story:sdk-crate created; its `Cargo.toml`, with
  `syn` feature `full`, stays story:sdk-crate's and is not edited here): the `tool` attribute.
  It replaces the function by a unit struct of the same name (the body moves into its executor) and
  emits `impl From<that struct> for
  (::inference::tools::Tool, Box<dyn ::inference::tools::ToolExecutor>)`, which `Agent::tool` takes
  (`impl Into<(Tool, Box<dyn ToolExecutor>)>`, story:sdk-agent). `ToolExecutor` is object-safe
  (`crates/inference-tools/src/executor.rs:24`).
- The attribute's `effect` takes `read_only` (the default), `idempotent_write` or
  `non_idempotent_write`, the three variants of `SideEffectClass`
  (`crates/inference-tools/src/lib.rs:67-74`); there is no `irreversible`. `confirm` sets
  `requires_confirmation`.
- JSON schema of the arguments through `schemars` (Q3 default). The generated arguments struct
  carries `#[serde(crate = "::inference::__private::serde")]` and
  `#[schemars(crate = "::inference::__private::schemars")]`, so its derives resolve through
  `inference` and not through a `serde` or `schemars` dependency of the application. Every other
  generated path is `::inference::tools::…` or `::inference::__private::{serde_json, tokio, exit}`
  (story:sdk-crate's `lib.rs`).
- In the same file: the `main` attribute, re-exported as `inference::main` by story:sdk-crate's
  `lib.rs`. The user writes `#[inference::main] async fn main(host: impl AudioHost, stack: Stack)
  { … }`, with no return type; `?` works inside the body. The macro emits:
  - `pub async fn run(host: impl ::inference::audio_device::AudioHost, stack: ::inference::Stack)
    -> ::inference::Result<()>` holding the body followed by `Ok(())`, under
    `#[allow(unreachable_code)]` (the seam tests call);
  - a real `fn main() -> ::std::process::ExitCode` that builds a multi-thread runtime with
    `::inference::__private::tokio::runtime::Builder::new_multi_thread().enable_all()` and returns
    `::inference::__private::exit(runtime.block_on(async { run(::inference::audio_device::CpalHost,
    ::inference::Stack::from_env()?).await }))`, so an error prints through `Display` (T7). It is
    marked `#[allow(dead_code)]` so a test that includes an example as a module
    (story:sdk-examples) compiles under `-D warnings`. `CpalHost` is a unit struct
    (`adapters/inference-audio-device/src/cpal_host.rs:31`). The runtime is multi-thread because a
    hosted adapter may block its calling thread while it connects: the AWS binding waits on a
    channel for its own pool (`adapters/inference-aws/src/duplex.rs:169-190`, called from
    `open` at `:331`).
  A signature other than `async fn main(<host>, <stack>)` is a `compile_error!` naming the
  expected one. The expansion needs the feature `audio-device` (`CpalHost`, behind `inference-audio-device`'s `device`), as every example on
  devices has.
- `crates/inference/src/macro_selftest.rs` (the private module story:sdk-crate declared, holding
  only its doc comment until now): a `#[cfg(test)]` unit test inside the `inference` crate that
  uses `#[inference::tool]`, so the `::inference::` paths resolve through
  `extern crate self as inference;`, and the `no_run` doc test of `#[inference::main]` below.
- `crates/inference/tests/tool_macro.rs` and `crates/inference/tests/ui/` (new, `trybuild` cases
  for `tool`).

## Acceptance

- A tool with no arguments, one with two typed arguments and one returning `Result` each produce the
  `Tool` a hand-written definition gives (name, description from the doc comment, schema,
  `side_effect`, `requires_confirmation`), compared in `tests/tool_macro.rs`.
- The generated executor runs the function: for the tool with two typed arguments, a call proposed
  on a committed span with JSON arguments, authorized and started (as
  `crates/inference-tools/tests/lifecycle.rs:52-76` builds one), handed to the executor's
  `execute(&call, deadline, &CancellationSignal::new())`, returns `Ok` with the function's result
  as `Json`. The same call with a malformed argument (a string where the function takes a number)
  returns `Err(ToolFailure)` of kind `ToolFailureKind::ExecutionError`
  (`crates/inference-tools/src/lib.rs:114-121`), and the function body does not run (a static counter its body increments stays unchanged). The tool
  returning `Result` maps its `Err` to `ExecutionError` too.
- A macro tool marked `effect = "non_idempotent_write"` is refused by
  `ToolCall::propose_speculative_as` with `ProposeSpeculativeError::SideEffecting`
  (`crates/inference-tools/src/call.rs:190-204`), and the same tool with no `effect` is proposed.
- A macro tool marked `confirm` is held unconfirmed by `ToolAuthority` until the
  `AuthorityHandle` confirms it (`crates/inference-pipeline/src/nodes/tool_authority.rs:16-19`),
  then runs. The test declares its own payload implementing `ToolPayload`, as
  `crates/inference-runtime/tests/tool_nodes.rs:64` does, and does not use story:sdk-payload's.
- `trybuild`: a compile-fail case shows the error for a non-`Deserialize` argument; the pass cases
  for `tool` keep an empty `fn main() {}`.
- The `macro_selftest.rs` unit test asserts the derived `Tool`'s name and argument schema and runs
  one `execute` round trip (JSON arguments in, the function's result out, as above); it passes
  under `cargo test -p inference --lib macro_selftest`.
- `#[inference::main]` compiles in a `no_run` doc test in `src/macro_selftest.rs`: the doc test holds
  `#[inference::main] async fn main(host: impl AudioHost, stack: Stack) { let _ = (host, stack); }`
  and its `use` lines, and is gated `#[cfg_attr(feature = "audio-device", doc = …)]` so it is
  compiled only with `audio-device`; `no_run` keeps it from opening devices or reading the
  machine's configuration.
- A `trybuild` compile-fail case (gated `#[cfg(feature = "audio-device")]`): `#[inference::main]` on
  a `main` without the `stack` parameter fails with the message naming the expected signature.
- In `tests/tool_macro.rs`, gated `#[cfg(all(feature = "testing", feature = "audio-device"))]` (`testing` gives `FakeHost` but not `CpalHost`, which the emitted `fn main` names): a module `app` holding
  `#[inference::main] async fn main(host, stack)` whose body asserts inside it: `stack.adapter(
  Role::Llm)` is the `mock` adapter, and `host.devices()` returns the devices the test gave the
  `FakeHost`. The test calls `app::run(FakeHost::new(..), stack)`, with `FakeHost` as
  `inference::audio_device::fake::FakeHost` (`adapters/inference-audio-device/src/lib.rs:32`) and
  the stack built with `Stack::bind(Binding::{stt, llm, tts, speech}(<mock>))` for each role
  (story:sdk-stack); `run` returning `Ok` means the asserts held. A second body that ends in
  `Err(..)?` makes `run` return that error.
- `rg -n 'serde|schemars|tokio|inference_tools|audio_device|CpalHost' crates/inference-macros/src/lib.rs | rg -v '::inference::'`
  exits 1: every such path in the `tool` and `main` expansions goes through `::inference::`.
- `rg -n 'serde\(crate' crates/inference-macros/src/lib.rs` and
  `rg -n 'schemars\(crate' crates/inference-macros/src/lib.rs` each match.
- `cargo test -p inference --test tool_macro` passes, as do
  `cargo test -p inference --features testing,audio-device --test tool_macro` and
  `cargo test -p inference --features audio-device --doc`.

Its diff lists none of `crates/inference/src/lib.rs`, `crates/inference/src/prelude.rs` and
`crates/inference/Cargo.toml` (story:sdk-crate wrote them once, decision D9 of `api-design:sdk`).

## Out of scope

MCP tools (`docs/design/inference.md:857`).
