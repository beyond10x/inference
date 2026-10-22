---
format: aep.planning-md/3
id: story:sdk-testing
kind: story
status: draft
title: inference::testing drives the production code path from a scripted stack
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-agent
- depends_on: story:sdk-tool-macro
- depends_on: story:sdk-usage-cost
scope:
- confidence: inferred
  path: crates/inference/src/testing.rs
- confidence: inferred
  path: crates/inference/tests/testing.rs
revision: 13
---
## Context

`docs/design/sdk.md` §6: `inference::testing`, the scripted stack, fake clock and fake audio host
behind one module, driving the production recipe, session and event mapping. Target T4, with
story:sdk-examples. Serves O4.

Ordered after story:sdk-tool-macro (the §6 test uses `#[inference::tool]` and `Agent::tool`) and
story:sdk-usage-cost (it reads `agent.usage()`).

## Domain relations

No new noun: a scripted stack is an `inference.sdk.Stack` whose bindings are the testkit mocks'
adapters.

## Planned scope

- `crates/inference/src/testing.rs` (feature `testing`): `Stack::scripted()` with `hears`, `says`,
  `calls`, `fails` (the cascade roles) and `speech(model: MockSpeechModel)` (the duplex mock for the
  `speech` role, built by the test, `MockSpeechModel::native(<turns>)` with or without
  story:sdk-duplex-node's `.closes()`), each binding the testkit mock its role takes through
  the typed constructor of that role (`Binding::stt(MockRecogniser)`, `Binding::llm(MockGenerator)`,
  `Binding::tts(MockSynthesiser)`, `Binding::speech(MockSpeechModel)`, each passed to `Stack::bind`, story:sdk-stack; the `mock` registration is
  that story's `crates/inference/src/registry/mock.rs`, and this story adds none);
  `FakeClock`, implementing `inference::Clock` (story:sdk-agent), with `FakeClock::at("HH:MM")`;
  `assert_events!`; a re-export of `FakeHost` (`adapters/inference-audio-device/src/fake.rs:114`, whose module is unconditional), reached through `testing`'s `audio-host` with no cpal (story:sdk-crate).
  The reply-audio decoder (R105): `inference::testing::mark`, a re-export of `inference_testkit::mark`
  (story:sdk-agent: `decode(&AudioFrame) -> Option<Mark>`, `MockId::{Speech, Synthesiser}`), so a
  test tells which mock and which reply a played frame came from through `FakeHost::played_frames()`.
  `assert_events!(events, [Variant { field: <expr>, .. }, ..])`, with `events` the `Events` that
  `agent.events()` returned (story:sdk-agent; the agent itself is not a stream), awaits its next events in
  order: its expansion reads each with `.await` on that stream, so it is used inside an
  `async` test and the call site writes no `.await` (`docs/design/sdk.md` §6); for each it checks the variant, binds each named field by reference
  (`let Event::Said { text, .. } = &event else { panic!(..) }`) and asserts `*text == <expr>`. A
  `String` field therefore matches a `&str` literal through std's `impl PartialEq<&str> for
  String`, with no `.to_string()` at the call site; any other field type compares through its own
  `PartialEq`. A mismatch panics with a message naming the event's position, the variant and the
  field (``assert_events!: event 2 (Said): field `text` was "..", expected ".."``).
- Re-exports under `inference::testing` of every test double the ported examples use
  (story:sdk-examples), each from where it lives today:
  - `Mocks`, `TurnStore`, `CascadeRefused` (`crates/inference-testkit/src/cascade.rs:284`, `:259`,
    `:341`) and `LateSynthesiser` (`src/cascade/late.rs:60`);
  - `FanOut` (`src/stt/fan_out.rs:38`);
  - the modules `stt`, `llm`, `tts` and `duplex` of the testkit as `inference::testing::{stt, llm,
    tts, duplex}`, which carry the three `MockConfiguration` enums (`src/stt.rs:112`,
    `src/llm.rs:76`, `src/tts.rs:113`, one per module, so none is re-exported at the top),
    `MockRecogniser`, `MockGenerator`, `MockSynthesiser`, `Cue`, `ScriptStep`,
    `stt::synthetic_audio` and `duplex::mock::{MockSpeechModel, Reply, ScriptedTurn,
    AUDIO_FRAMES}` (`src/duplex/mock.rs:50-117`).
  `StreamEvent` and `RunLog` are reached through `inference::pipeline::nodes::log` after
  story:sdk-model-nodes, not through `testing`.
- `crates/inference/tests/testing.rs` (stub from story:sdk-crate, filled here).

## Acceptance

All in `crates/inference/tests/testing.rs`, passing in
`cargo test -p inference --features testing --test testing`:

- The test of `docs/design/sdk.md` §6 passes as written (its `time_of_day` tool returns a fixed
  string; no clock crate is a dependency).
- `Agent::voice_with` over `Stack::scripted().speech(MockSpeechModel::native(<one turn>).closes())`
  (the duplex-mock stack) and over the same `speech` with `.hears(..).says(..)` (the mixed stack,
  cascade roles and `speech` all bound) each reports the recipe `native_voice` in `describe()` and,
  driven by `FakeHost`, yields `Event::Heard`, the input `Event::Usage`, `Event::Said` with the
  script's text and the response's `Event::Usage`, in that order (the mapping table of
  story:sdk-agent; the audio goes to the host, not the stream), then `Event::Closed` once the
  script is spent (R167).
- What was played, with no `pull` (R105, R106): in each run of the case above, no task calls
  `FakeHost::pull`, `run_on_devices_with` returns, and `FakeHost::played_frames()` decoded with
  `inference::testing::mark::decode` holds every frame of the reply in order; for the mixed stack
  every decoded frame is `MockId::Speech` and none `MockId::Synthesiser`.
- `assert_events!` with a `String` field and a `&str` literal: over a stack that `says("hi")`,
  `assert_events!(events, [Said { text: "hi", .. }])`, `events` from `agent.events()`, passes; the same with `"bye"` panics, in a
  test marked ``#[should_panic(expected = "field `text`")]``, so the message names the field.
- `FakeClock::at("10:30")`: the agent's `Event::Started` carries 10:30.
- Each re-export above compiles from `inference::testing` in a test that names it.
- `rg -n 'cfg\(feature = "testing"\)' crates/inference/src/agent.rs crates/inference/src/recipe.rs crates/inference/src/recipe`
  exits 1: the `testing` feature does not leak into the agent or the recipes (R208). Their
  `#[cfg(test)]` unit-test modules (story:sdk-agent's in `agent.rs`, story:sdk-recipes' in
  `recipe.rs` and `src/recipe/`) are allowed, and the pattern does not match them.
- A scripted provider failure (`fails`) surfaces as `Event::Error` then `Event::Closed`, and
  `agent.usage()` still holds the usage received before it.
- T4 is shared with story:sdk-examples: this story gives the scripted stack, the fake clock and the
  fake host; story:sdk-examples drives its T1 example through them (`run(host, stack)`).

Its diff lists none of `crates/inference/src/lib.rs`, `crates/inference/src/prelude.rs` and
`crates/inference/Cargo.toml` (story:sdk-crate wrote them once, decision D9 of `api-design:sdk`).

## Out of scope

Recording and replay of real sessions (`crates/inference-runtime/src/replay.rs`); a later story.
