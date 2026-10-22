---
format: aep.planning-md/3
id: story:sdk-one-call
kind: story
status: draft
title: transcribe, chat, speak and talk work in one call, with Chat, Transcriber and Voice handles
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-agent
scope:
- confidence: inferred
  path: crates/inference/src/chat.rs
- confidence: inferred
  path: crates/inference/src/oneshot.rs
- confidence: inferred
  path: crates/inference/src/transcriber.rs
- confidence: inferred
  path: crates/inference/src/voice.rs
- confidence: inferred
  path: crates/inference/tests/oneshot.rs
revision: 11
---
## Context

`docs/design/sdk.md` §3.1 and the handle table of §3.2: the one-call functions `transcribe`, `chat`,
`speak`, `talk`, and the handles `Chat`, `Transcriber`, `Voice`, each with `describe()` and the
escape methods (`docs/design/sdk.md` §3.5). Targets T5 for the three handles. Serves O1: one adapter
used alone.

## Domain relations

No new noun: each is an `Agent` over the `chat`, `transcribe` or `speak` recipe.

## Planned scope

- `crates/inference/src/oneshot.rs`: `transcribe(path)`, `chat(prompt)` and their stack forms
  `transcribe_with(&stack, path)` and `chat_with(&stack, prompt)`; behind `audio-device`, `speak(text)` and
  `talk()` on the default host, and behind `audio-host` `speak_with(&stack, text, host)` and `talk_with(&stack, host)` over any `AudioHost`
  (decision D18 of `api-design:sdk`: the stack first, then the plain form's arguments, then the
  host). Each plain form is its `_with` form over `Stack::from_env()?` (and, for `speak` and `talk`,
  the default host); a `_with` form uses the stack as given and resolves nothing more. All are
  public and named in `docs/design/sdk.md` §3.1 and decisions D12 and D18. Each opens its session
  on `inference::sessions()`, which story:sdk-agent defines.
  - `transcribe(path)` reads the WAV file with `hound` (a normal dependency story:sdk-crate
    declared) and feeds its frames to the `transcribe` recipe.
  - `talk` and `talk_with` run through story:sdk-agent's crate-private `run_until(host, stop)` with
    `stop` = `tokio::signal::ctrl_c()`, so they end when the session ends or on Ctrl-C
    (`docs/design/sdk.md` §3.1, "until Ctrl-C").
- `crates/inference/src/chat.rs`, `src/transcriber.rs`, `src/voice.rs`: `Chat`, `Transcriber`,
  `Voice`, each built by `new()` or by `new_with(&stack)` (D18; `new()` is `new_with` over
  `Stack::from_env()?`), each with `session()`, `pipeline()` and `describe()`; `Chat::tool`, taking the same
  `impl Into<(Tool, Box<dyn ToolExecutor>)>` as `Agent::tool` (the design's handle table in §3.2
  gives `Chat` tools). The data methods of the handle table:
  - `Chat::send(text) -> Result<String>` and `Chat::stream(text)`, a stream of text deltas;
  - `Transcriber::transcribe_frames(frames: impl IntoIterator<Item = AudioFrame>) ->
    Result<Vec<inference::text::Span>>`, audio frames in, committed spans out, and
    `Transcriber::transcribe_file(path)`, the same over a WAV file read with `hound`;
  - `Voice::synthesise(text) -> Result<Vec<AudioFrame>>`, text in, frames returned, and, behind
    `audio-host`, `Voice::play(text, host: impl AudioHost)`, text in, frames played.
  Each of `chat.rs`, `transcriber.rs` and `voice.rs` carries a compile-time `Send` assertion for its
  handle (`const _: fn() = || { fn assert_send<T: Send>() {} assert_send::<Chat>(); };`, and the
  same for `Transcriber` and `Voice`), as `docs/design/sdk.md` §8 promises.
- `crates/inference/tests/oneshot.rs` (stub from story:sdk-crate, filled here), over the testkit
  mocks bound with `Stack::new().bind(Binding::<role>(<mock>))` (story:sdk-stack's typed constructors; this
  story adds no registration) and handed to the `_with` forms, and `FakeHost` (`adapters/inference-audio-device/src/fake.rs:122`). Its WAV
  fixtures are written by the tests with `hound` into a `tempfile::tempdir()` (both declared by
  story:sdk-crate); no WAV file is committed.

## Acceptance

All in `crates/inference/tests/oneshot.rs`. Every test in the file first takes one file-level
`tokio::sync::Mutex<()>`, so no two of them open sessions at once and the row counts below are
exact. Every test builds its stack with `Stack::new().bind(Binding::<role>(<mock>))` and calls
the `_with` form or `new_with`; none calls a plain form, so none reads or sets the process
environment:

- `transcribe_with(&stack, path)` returns the committed text of a WAV file the test wrote with
  `hound` into a temporary directory; `chat_with(&stack, prompt)` returns the scripted generator's
  text; `Chat::new_with(&stack)?.stream(..)` yields the deltas whose concatenation is that text;
  `speak_with(&stack, text, host)` plays on the fake host with no `pull` (R106): `FakeHost::played_frames()`, decoded with
  `inference_testkit::mark::decode` (story:sdk-agent; the testkit dev-dependency), is every frame of the
  scripted synthesiser's reply, each `MockId::Synthesiser`, in order.
- `talk_with(&stack, host)`, over a stack binding `speech` to
  `MockSpeechModel::native(<one turn>).closes()` (story:sdk-duplex-node), returns `Ok` once the
  script is spent and the mock closed the session, after which `FakeHost::input_open()` and
  `FakeHost::output_open()` (`fake.rs:182`, `:188`) are both false.
- The plain forms are the `_with` forms over `Stack::from_env()`: `rg -n 'Stack::from_env\(\)'
  crates/inference/src/oneshot.rs crates/inference/src/chat.rs crates/inference/src/transcriber.rs
  crates/inference/src/voice.rs` matches once per plain function and `new()`, seven lines.
- Each function opens and closes exactly one session: after each call, `inference::sessions()
  .sessions_view()` (`live.rs:561`) has exactly one row more than before it, and that row's `state`
  is `SessionState::Closed` (`crates/inference-facade/src/session.rs:472`, `:80`).
- `Chat` keeps history: a second `send` sees the first turn in the generator's request.
- `Chat::tool`, given a hand-built `(Tool, Box<dyn ToolExecutor>)` pair (no macro needed): with a
  scripted generator that calls the tool, the executor receives the call
  (its arguments as scripted) and the reply contains the executor's answer.
- `Transcriber::transcribe_frames` over the frames of
  `inference_testkit::stt::synthetic_audio::tone_silence_tone_silence()`
  (`crates/inference-testkit/src/stt/synthetic_audio.rs:114`, through the testkit dev-dependency
  story:sdk-crate declared; `inference::testing` does not exist until story:sdk-testing)
  returns the spans the mock recogniser scripts, in order, each in `SpanState::Committed`.
- `Voice::synthesise(text)` returns the mock synthesiser's frames for `text`; `Voice::play(text,
  FakeHost::new(..))` plays them with no `pull`: `FakeHost::played_frames()` equals the frames `synthesise` returned.
- `Chat`, `Transcriber` and `Voice` each: `describe()` names the recipe (`chat`, `transcribe`,
  `speak`); `session()` and `pipeline()` return objects on which one method is called.
- `rg -n 'assert_send::<(Chat|Transcriber|Voice)>\(\)' crates/inference/src` matches three lines,
  and `cargo check -p inference --all-features` exits 0 with them.
- `cargo test -p inference --features testing --test oneshot` passes.

Its diff lists none of `crates/inference/src/lib.rs`, `crates/inference/src/prelude.rs` and
`crates/inference/Cargo.toml` (story:sdk-crate wrote them once, decision D9 of `api-design:sdk`).

## Out of scope

A blocking API (`docs/design/sdk.md` §8); `usage()` and `cost()` (story:sdk-usage-cost); the
Ctrl-C path's own unit test (story:sdk-agent's `run_until` test).
