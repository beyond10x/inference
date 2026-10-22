---
format: aep.planning-md/3
id: story:sdk-examples
kind: story
status: draft
title: A voice agent with a tool is 15 lines, and the voice examples run on the SDK
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-one-call
- depends_on: story:sdk-tool-macro
- depends_on: story:sdk-testing
- depends_on: story:sdk-usage-cost
- depends_on: story:realtime-voice-demo
scope:
- confidence: inferred
  path: examples/local-voice-cascade
- confidence: inferred
  path: examples/realtime-voice
- confidence: inferred
  path: examples/sdk-voice
revision: 20
---
## Context

`docs/design/sdk.md` §2, T1, T2, T3 and, with story:sdk-testing, T4: the proof that the SDK pays
off is an application. A new example package `sdk-voice` (`examples/sdk-voice`) holds the design's
§3.2 program; `realtime-voice` and `local-voice-cascade` are ported onto the SDK. Serves O1 and O5.

## Domain relations

No new noun.

## Planned scope

- `examples/sdk-voice/` (new), package name `sdk-voice`: `Cargo.toml` whose `[dependencies]` are
  `inference` (features `audio-device` and `cost`) and `chrono` (the tool's clock), with no `serde`,
  `schemars`, `serde_json` or `tokio`, and whose `[dev-dependencies]` are `inference` with `testing`
  and `tokio` (features `macros`, `rt-multi-thread`), so the T2 tests run as
  `#[tokio::test(flavor = "multi_thread")]` (decision D16's runtime);
  `examples/agent.rs` (the §3.2 program as printed, with one tool), `examples/minimal.rs` (the same
  without the tool and `.instructions`, whose count §3.2 gives), `tests/`. Each example is one
  `#[inference::main] async fn main(host: impl AudioHost, stack: Stack)` with no return type
  (story:sdk-tool-macro), which emits the real `fn main` (default host `CpalHost`,
  `Stack::from_env()`) and the `pub async fn run(host, stack) -> inference::Result<()>` the tests
  call. To reach `run`, each test file includes its example as a module
  (`#[path = "../examples/agent.rs"] mod agent;`). The root `Cargo.toml` picks the crate up through
  `examples/*` (`Cargo.toml:3`).
- `examples/realtime-voice/`: its configuration functions (`src/lib.rs:110` `openai_config`, `:129`
  `openai_live_config`, `:147` `nova_config`), its event loop (`:365` `pump_microphone`, `:381`
  `pump_events`, `:395` `feed_speaker`) and the provider `match` (`examples/talk.rs:82`) replaced by
  SDK calls and one stack file per provider (`stacks/openai.toml`, `stacks/openai-live.toml`,
  `stacks/nova.toml`), each naming `profile = "hybrid_opt_in"`, because a stack with no
  `profile` key declares `offline_strict`, which denies a hosted adapter (story:sdk-stack, R201).
  The provider values of the three configuration functions (voice, formats,
  turn detection, `auto_response`, `auto_interrupt`, idle limit) are not rewritten here: they move
  to story:sdk-stack's registry descriptors (`registry/openai.rs`, `registry/aws.rs`), and the
  example gives only what is its own, `INSTRUCTIONS` and the tool, through the builder.
  `local_time` becomes an `#[inference::tool]`. `src/lib.rs` exposes
  `pub fn agent(stack: Stack)`, the `Agent` builder `talk` runs (its instructions and `local_time`),
  so `examples/compare.rs` takes the same session configuration from it. Kept,
  because `src/compare.rs:50` and `tests/compare.rs:27-30` use them: `src/compare.rs`, `Note`,
  `Ending`, `pcm16` (`src/lib.rs:75`, `:189`, `:227`) and `src/usage.rs` (`Reported`, `Tally`,
  `Totals`, `Usage`, `Counts`). `realtime-voice` keeps its whole tally, text-token pricing included,
  while Q5 stands: the SDK's cost leaves audio tokens unknown (`docs/design/sdk.md` §7); the SDK's
  text-token pricing is checked by story:sdk-usage-cost's fixture. The `use` lines of
  `src/compare.rs` (`:44-49`) and `src/usage.rs` (`:30`) move from `inference_audio_device::` and
  `inference_duplex::` to `inference::audio_device::` and `inference::duplex::`.
- story:realtime-voice-demo (active) also owns `examples/realtime-voice`, so this story depends on
  it (R122) and ports the package as that story leaves it.
- `examples/realtime-voice/Cargo.toml`: `[dependencies]` hold `inference` (features `openai`,
  `openai-live`, `aws`, `audio-device`), `chrono` and `serde_json`; `[dev-dependencies]` hold
  `inference` with `testing`, `tokio` (features `macros`, `rt-multi-thread`), and the crates the
  unchanged `tests/compare.rs` and the migrated `examples/compare.rs` name: `inference-duplex`,
  `inference-openai`, `inference-aws` (the last two also for the provider checks in
  `tests/wiring.rs`). Each rewritten test that awaits the agent is
  `#[tokio::test(flavor = "multi_thread")]`.
- `examples/realtime-voice/examples/compare.rs`: its import of `nova_config`, `openai_config` and
  `openai_live_config` (`:35`), which this story deletes, goes. Each provider's `SessionConfig`
  comes from `realtime_voice::agent(<stack>).describe()` (story:sdk-agent's builder `describe()`,
  which opens no session) over that provider's stack file, resolved with `Stack::resolve_with`.
  The models stay built through the provider crates (`:27-33`, `:127-166`), because the kept
  `realtime_voice::compare::run` is generic over `NativeSpeechModel` and the OpenAI Live billing
  reads `OpenAiLiveSession::voice_seconds` (`:143`) from the concrete session type.
- `examples/realtime-voice/tests/`: `wiring.rs`, `usage.rs`, `live_usage.rs` and
  `adversary2_nova_barge_in.rs` rewritten in place (see Acceptance); `compare.rs` untouched.
- `examples/local-voice-cascade/`: its testkit imports (`src/lib.rs:108-117`, `src/local.rs:62-68`)
  replaced by `inference::testing` (story:sdk-testing's re-exports) and `inference::pipeline`, its
  hand declaration by `Recipe::voice_cascade()` (R190, R191). `declare`, `declare_requiring`,
  `CascadeHandles` and the `local` module's public items keep their signatures:
  - `declare_requiring` (`src/lib.rs:515`) holds the hand declaration today (`:556-660`) and keeps
    its public signature. It still builds the `offline_strict` profile admitting `admitted` and the
    `Fixture` catalog (`:519-526`), then calls
    `Recipe::voice_cascade().declare_into(declaration, &catalog, profile_id, &[(Role::Stt, <adapters[0]'s id>), (Role::Llm, <adapters[1]'s>), (Role::Tts, <adapters[2]'s>)], &[(Role::Stt, stt_requires)])`
    (story:sdk-recipes) in place of the hand `declaration.build(..)`, and validates and plans the
    returned pipeline as today (`:674-680`).
  - An `Err` from `declare_into` cannot occur there (R200). `profile_id` is the id of the profile
    the `Fixture` holds, whose `profile` answers it (`:436-438`); the three adapter ids are those of
    the `adapters` the `Fixture` holds, whose `adapter` finds them (`:440-444`); and the three pairs
    cover every role `voice_cascade` needs. So `declare_requiring` takes the result with
    `.expect("the Fixture catalog holds the profile and the three adapters declare_into is given")`.
    Admission stays planning's and surfaces as `CascadeRefused::Unplannable`, as today
    (`tests/local_adapters.rs:1397-1420`); `CascadeRefused` keeps its two variants, `Invalid` and
    `Unplannable` (`crates/inference-testkit/src/cascade.rs:341-346`). `declare_over` (`:493`) and `declare` (`:458`) keep
    calling it.
  - `CascadeIds` is read off the caller's declaration by the fixture's node and port names, which
    `voice_cascade` keeps (story:sdk-recipes), through `Declaration::nodes()` and `ports()`
    (`crates/inference-pipeline/src/declaration.rs:708`, `:713`), keeping only the entries whose
    `pipeline_id()` is the returned one, since the caller's declaration may hold other pipelines.
  - The sink is the recipe's `speaker` node (`NodeRole::Sink`, `src/lib.rs:566`), with its
    `turns` port (`:602`). `nodes_over` (`:844`) builds its implementation as
    `AcknowledgingSink::over(run_id, Box::new(speaker)).with_turns(ids.sink_turns)`, the `Playout`
    for `Speaker` and the constructor story:sdk-recipes adds (R181), the same sink as today's
    `new`. So `CascadeHandles.sink` stays an `AcknowledgingHandle`, and the barge-in tests read
    the same interruptions, stale frames and pushed counts.
  - The other `CascadeHandles` fields come from the node implementations `nodes_over` builds for
    the recipe's nodes, as today (`:860-882`): `policy` from `turn policy`, `stt` from `stt`,
    `input_gate` and `output_gate` from `input gate` and `output gate`, `llm` from `agent`, `tts`
    from `tts`, each through its `handle()`. `VoicePayload` stays the alias
  `pub type VoicePayload = inference::Payload;` story:sdk-payload left, so its tests compile
  unchanged. `Cargo.toml`:
  - `[dependencies]`: `inference` with `features = ["testing"]`, because `src/` reaches the
    testkit through `inference::testing` (`testing` implies `audio-host`, which gives the
    `inference::audio_device` that `src/lib.rs` uses in place of today's
    `inference-audio-device` with `default-features = false` and `pipeline`, `Cargo.toml:16`);
    `inference::core`, `audio`, `text`, `stt`, `llm`, `tts` and `pipeline` need no feature. The
    three adapter crates stay optional, as today, behind the package's own `local` feature, which
    enables none of `inference`'s. `audio-host` leaves out `inference-audio-device`'s `device`
    (cpal), as today's `default-features = false` does, so the normal build gains no cpal.
  - `[dev-dependencies]`, the crates its unchanged tests and `examples/talk.rs` name, with the
    features they use today: `inference-testkit` (`llm`, `run`, `stt`, `tts`, as `Cargo.toml:21`),
    `inference-audio-device` (`pipeline`, defaults on, as `Cargo.toml:26`), and `inference-audio`,
    `inference-core`, `inference-pipeline`, `inference-runtime`, `inference-text`, `inference-tts`
    with no features (none of them declares any); `clap` stays as it is.

## Acceptance

- T1: the count is the non-blank, non-comment lines of the whole example file, the `use` line, the
  attributes and the tool function included:
  `grep -cvE '^[[:space:]]*($|//)' examples/sdk-voice/examples/agent.rs` prints at most 15 and the
  same for `examples/minimal.rs` at most 10. A test in `examples/sdk-voice/tests/` asserts the same
  count in Rust (the files hold no block comment). `agent.rs` keeps `.instructions` and ends with
  `println!("{}", agent.usage());`. For the §3.2 text, which `rustfmt --check` accepts unchanged,
  the command prints 15 and 7.
- T2: `examples/sdk-voice/tests/`, each test `#[tokio::test(flavor = "multi_thread")]`, drives
  each example's generated `run` with a clone of a `FakeHost` (`inference::testing::FakeHost`)
  under three scripted stacks, the example source being the same in each run: cascade
  (`Stack::scripted().hears(..).says(..)`), duplex
  (`Stack::scripted().speech(MockSpeechModel::native(<one turn>).closes())`) and mixed (both, the
  same `speech`). The duplex and mixed runs end when the mock closes the session
  (story:sdk-duplex-node). A cascade session closes only when the application closes it, so the
  cascade test spawns `run`, waits until `FakeHost::played_frames()` holds every frame of the reply (no
  `pull`, R106), then aborts the task. Input, the same for
  each stack: the host is `FakeHost::new` with a 16 kHz mono default input (`FakeDevice::input`,
  `adapters/inference-audio-device/src/fake.rs:30`) and the laptop's 48 kHz stereo output. Once the
  input stream runs, the test feeds
  `inference::testing::stt::synthetic_audio::synthesize(&FIXTURE[..2])` (`synthetic_audio.rs:77`,
  `:85`): 300 ms of tone, then 500 ms of silence, PCM s16le at 16 kHz mono in frames of 160
  samples. Each frame goes to one `FakeHost::feed` call (`fake.rs:143`), its samples divided by
  32768 to the device's `f32`. The speech then silence ends the user's turn: the duplex mock's
  turn detection, and the cascade's endpoint controller, for which this fixture is made. For each
  stack:
  - the recipe: `Agent::voice_with(<the same scripted stack, built again>).describe()` names
    `voice_cascade`, `native_voice` and `native_voice` respectively. The test then starts that agent
    (`.start().await?`), reads `agent.session().session_id()`, and asserts that the
    `inference::sessions().sessions_view()` row carrying that id has the matching `mode`
    (`InteractionMode::TurnBased`, `Duplex`, `Duplex`, `crates/inference-pipeline/src/lib.rs:199`).
    It finds the row by session id only: no lookup by mode, no row count;
  - the devices: `FakeHost::played_frames()` on its clone, decoded with `inference::testing::mark::decode`
    (R107), holds every frame of the scripted reply in order; for the cascade each decoded frame is
    `MockId::Synthesiser`, and for the duplex and mixed stacks each is `MockId::Speech`, none
    `MockId::Synthesiser`, so the mixed run's audio is the native model's, not the cascade's;
  - the printed usage: the test re-runs its own binary (`std::env::current_exe()`) with
    `--exact <helper> --ignored --nocapture` and the stack's name in an environment variable, and
    asserts that the child exited 0 and that its stdout has a line equal to this one. Duplex and mixed:
    `80 input tokens, 41 output tokens, cost: unknown (no prices)`, the duplex mock's fixed counts
    for one turn (transcription 10 in and 3 out, `crates/inference-testkit/src/duplex/mock.rs:434-436`;
    response 20 text and 50 audio in, 8 text and 30 audio out, `:498-507`). Cascade:
    `unknown input tokens, unknown output tokens, cost: unknown (no prices)`, because the scripted
    generator reports no usage (`crates/inference-testkit/src/llm.rs:276-279`). The `#[ignore]`d
    helper calls `run` over that stack. For the cascade it also waits until `FakeHost::played_frames()` holds the reply (no `pull`), then prints
    a ready line; on that line the parent sends the child SIGINT
    (`std::process::Command::new("kill").args(["-INT", &child.id().to_string()])`), which ends
    `run_on_devices_with` through its Ctrl-C path (story:sdk-agent), so `run` prints the line. The
    cascade case is `#[cfg(unix)]`.
- T4 (with story:sdk-testing): the same tests run under `cargo test -p sdk-voice` with no device,
  network or weights.
- T3: `examples/sdk-voice/Cargo.toml` has exactly one `[dependencies]` key starting with
  `inference`, no contract crate under `[dependencies]` (its `[dev-dependencies]` may list them),
  and none of `serde`, `schemars`, `serde_json`, `tokio` under `[dependencies]` (`tokio` is a
  dev-dependency for the T2 tests), so `cargo test -p sdk-voice`
  building `agent.rs` is the end-to-end check that the `tool` and `main` expansions reach every
  path through `::inference::` (story:sdk-tool-macro). `cargo tree -p realtime-voice -e normal
  --depth 1` lists no `inference-` crate, and `cargo tree -p local-voice-cascade -e normal --depth 1
  --features local` lists exactly `inference-llama-server`, `inference-sherpa-onnx` and
  `inference-whispercpp`, beside `inference` (`inference v` lines excluded from both counts). Those
  three stay because `src/local.rs` builds on their types directly and its tests import that API
  (`tests/local_adapters.rs:85`, `tests/adversary1_stt_swap.rs:21`, `examples/talk.rs:54`).
  `LocalRecogniser` (`local.rs:405`) wraps both recognisers in one `Recogniser` impl (`:440`)
  chosen per run by `INFERENCE_CASCADE_STT` (`SttBackend`, `:119`), and `loopback` (`:342`)
  refuses a server URL in llama-server's `Config` (`:277`) before `LlamaServer::new` (`:539`)
  opens anything. The registry's one-adapter-per-role binding by name expresses neither. T3
  therefore does not hold for `local-voice-cascade` (`docs/design/sdk.md` §2 records it).
- `cargo tree -p local-voice-cascade -e normal` lists no `cpal`, and neither does `cargo tree -p
  local-voice-cascade -e normal --features local` (`inference` with `testing`, which implies
  `audio-host` only; story:sdk-crate).
- `cargo test -p realtime-voice` passes, with `git diff` empty for `tests/compare.rs`, and
  `cargo build -p realtime-voice --examples` exits 0 with the migrated `examples/compare.rs`.
  `cargo test -p local-voice-cascade --features local` passes, with `git diff` empty for each of the
  five `tests/*.rs`.
- The four rewritten realtime-voice test files keep every test function's name and assertion,
  driven by `inference::Agent` over a stack binding `speech` to
  `inference::testing::duplex::mock::MockSpeechModel` and `run_on_devices_with(FakeHost)`, with
  `inference::Event` in place of `Note`. For each file,
  `rg -U -o -r '$1' '#\[(?:tokio::)?test\]\n(?:#\[ignore[^\n]*\]\n)?(?:async )?fn ([a-z0-9_]+)' <file>`
  lists the same test names before and after. The test carrying each assertion:
  - `wiring.rs`: all twelve tests in place under their names. The barge-in, truncation and
    nova-playout tests also have SDK counterparts in story:sdk-agent's `run_on_devices_with` test
    and story:sdk-duplex-node's `duplex_node.rs`. `the_time_tool_is_answered_by_call_id_and_its_answer_spoken`
    and `the_local_time_answers_a_time_and_an_offset` run over the `#[inference::tool]`
    `local_time`. `the_demo_sessions_are_what_each_provider_takes` and
    `the_openai_live_configuration_is_one_gpt_live_takes` take the `SessionConfig` from the
    builder's `describe()` (story:sdk-agent), which opens no session, over a stack resolved with
    `Stack::resolve_with` from each provider's stack file; they assert that
    `stack.adapter(Role::Speech)` is that provider's adapter, then make the calls they make today
    (`wiring.rs:442-468`, `:503-519`): the provider crate's `capabilities().admit(&config)` and
    the binding's `client_config(&config)`, with the same assertions on the result.
    `a_session_the_model_refuses_opens_nothing` binds `MockSpeechModel::server_turns(vec![])` with
    `Binding::speech`, which opens with story:sdk-stack's `default_session_config()` (24 kHz
    input), so the model still refuses it (`wiring.rs:483-501`) and the devices are released.
    `the_time_tool_is_answered_by_call_id_and_its_answer_spoken` rests on story:sdk-recipes'
    `native_voice` tool wiring: the mock's call reaches the `local_time` executor and is answered
    by its call id, and the events are `ToolCalled` then `Said`.
    The tests that open over `nova_config()` today (`wiring.rs:278`, `:322`, `:406`,
    `usage.rs:409`) build their agent with
    `Agent::voice().session_config(inference_aws::duplex::default_session_config())`
    (story:sdk-agent's builder; the export is story:sdk-stack's, the value the registry's `aws`
    descriptor uses), through the existing dev-dependency `inference-aws` with `duplex`
    (`Cargo.toml:20`). Acceptance: in
    `a_provider_side_barge_in_stops_playout_without_truncating`, the agent's `describe()`
    `SessionConfig` has 16 kHz input and `auto_interrupt` on, and the session opens over the
    `server_turns` mock, which refuses the 24 kHz default (`wiring.rs:483-501`).
  - `usage.rs`: `a_later_usage_of_a_response_replaces_the_earlier_one`,
    `transcription_usage_never_reported_stays_unknown`, `the_cost_of_a_fixed_table_is_exact` and
    `a_price_table_is_refused_when_it_is_not_one` keep their bodies. The six that open a
    `Conversation` are rewritten in place over `Event::Usage`; the per-handle sum is also
    story:sdk-usage-cost's `Agent` case.
  - `live_usage.rs`: `a_spoken_question_prints_its_usage_line_and_the_totals` in place, over the
    OpenAI stack file, still `#[ignore]` (billed).
  - `adversary2_nova_barge_in.rs`: `speech_over_a_nova_answer_still_playing_stops_playout` in
    place, over a Nova-shaped `MockSpeechModel` script; its `nova_config()` at `:125` becomes
    `inference_aws::duplex::default_session_config()` (R108, R92), the aws crate's export
    story:sdk-stack adds.
  No test function is deleted.
- `rg -n inference_testkit examples/realtime-voice/src examples/local-voice-cascade/src` exits 1
  (`examples/stt-only/src/lib.rs:10` is out of scope).
- The cascade is declared through the recipe inside `declare_requiring` (R190, R192):
  `rg -U -o 'pub fn declare_requiring\((?s:.)*?\n\}' examples/local-voice-cascade/src/lib.rs | rg -c 'Recipe::voice_cascade\(\)'`
  prints 1, the match running from the signature to the function's closing brace at column 0;
  the name-free
  `rg -n '\.(node\(|input::<|output::<|connect\(|connect_feedback\(|expose\(|bind\()|\.build\(&catalog' examples/local-voice-cascade/src/`
  exits 1 (0 matches) after the port (R198); today its 67 matches are all the hand declaration
  (`src/lib.rs:555-627`), none outside it, the same lines the variable-anchored form matched; and `rg -n 'AcknowledgingSink::new' examples/local-voice-cascade/src/` exits 1.
- `cargo test -p local-voice-cascade --features local --test local_adapters` passes with
  `tests/local_adapters.rs` unchanged: `declare_requiring` with `STREAMING_STT_REQUIRES` still
  returns `CascadeRefused::Unplannable` naming the node `stt` for whisper.cpp
  (`tests/local_adapters.rs:1397-1420`) (R191).
- `rg -n 'fn (openai|openai_live|nova)_config|fn pump_events|fn pump_microphone|fn feed_speaker|next_event' examples/realtime-voice/src/lib.rs`
  exits 1, and so does `rg -n 'match args.provider' examples/realtime-voice/examples/talk.rs`.
- `rg -n '^use inference_' examples/realtime-voice/src/compare.rs examples/realtime-voice/src/usage.rs`
  exits 1.
- `wc -l examples/realtime-voice/src/*.rs examples/local-voice-cascade/src/*.rs` after the port is
  reported beside the 0.7.0 numbers of `docs/design/sdk.md` §1; no target is set for it.

Its diff lists none of `crates/inference/src/lib.rs`, `crates/inference/src/prelude.rs` and
`crates/inference/Cargo.toml` (story:sdk-crate wrote them once, decision D9 of `api-design:sdk`).

## Out of scope

`repo-podcast` and `stt-only` stay on their current APIs; `realtime-voice`'s own tally while Q5
stands.
