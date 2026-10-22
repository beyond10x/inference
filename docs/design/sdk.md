# The `inference` SDK

Status: draft, 2026-10-21, revised 2026-10-22 after the plan critics. Plan: `epic:sdk`, `api-design:sdk`. Specification: `ess/domains/sdk.yaml`.

The SDK is the `inference` crate that `docs/design/inference.md` §5 already reserves ("Optional
ergonomic re-exports", line 245) and that does not exist yet. This document makes it more than
re-exports: one dependency, one import, and an application that does in ten lines what the examples
do in six hundred, without hiding any layer underneath.

## 1. Why: what an application writes today

Measured on `main` at `dc8e13e` (0.7.0).

| what the application writes | where | lines |
|---|---|---|
| a payload enum implementing seven payload traits so one graph can carry audio, turns, text, gates, speech and playback | `examples/local-voice-cascade/src/lib.rs:182-369` | 188 |
| eleven nodes and thirty-odd typed port declarations, by hand, for the reference cascade | `examples/local-voice-cascade/src/lib.rs:515-690` | 176 |
| a hand-rolled event loop: microphone to session, session events, model audio to speaker, barge-in bookkeeping | `examples/realtime-voice/src/lib.rs:254-605` | 352 |
| one configuration function per provider, chosen by a `match` in the binary | `examples/realtime-voice/src/lib.rs:110-160`, `examples/realtime-voice/examples/talk.rs:82-110` | 80 |
| usage tallying and pricing, beside the facade's own `cost` module | `examples/realtime-voice/src/usage.rs` (501), `examples/repo-podcast/src/prices.rs` (220) | 721 |

Four structural gaps sit under those numbers:

| gap | evidence |
|---|---|
| G1 the STT, LLM and TTS graph nodes exist only in the testkit; a production cascade imports them from `inference-testkit` | `crates/inference-testkit/src/stt/node.rs`, `tts/node.rs`, `cascade/speech.rs:775`, `cascade/agent.rs`; imported at `examples/local-voice-cascade/src/lib.rs:108-117` |
| G2 no graph node wraps a `NativeSpeechModel`, so a native speech model never runs under the runtime or a session; `realtime-voice` drives the handle directly | `crates/inference-duplex/src/contract.rs:338`; no `impl NodeImplementation` over it (`grep`, 2026-10-21) |
| G3 no example uses the session facade | `grep -c inference_facade examples/*/src/*.rs`: 0 in every file |
| G4 every payload trait is implemented per application; the workspace ships no payload that implements them all | the seven `impl … for VoicePayload` at `examples/local-voice-cascade/src/lib.rs:201-338` |

The contracts, the runtime and the session are sound and stay as they are. What is missing is the
layer that assembles them, so every application assembles them again.

## 2. Targets

Each target is checkable, and each story of `epic:sdk` names the ones it moves.

| id | target | check |
|---|---|---|
| T1 | a voice agent on microphone and speaker, with barge-in and its usage printed (with the cost when a price document is named), in at most 10 lines without a tool and at most 15 with one | in a test, the non-blank, non-comment lines of the whole example file, the `use` line, the attributes and the tool function included (`grep -cvE '^[[:space:]]*($\|//)' <file>`): at most 10 in `examples/sdk-voice/examples/minimal.rs`, at most 15 in `examples/sdk-voice/examples/agent.rs` |
| T2 | the same binary runs a cascade (STT, LLM, TTS) or a native speech model, chosen by configuration alone | the T1 example passes its tests under three stacks with no source change |
| T3 | one dependency: `inference = { version, features = [...] }`; no application lists a contract crate | the SDK examples' `Cargo.toml` has one `inference*` key under `[dependencies]` and no contract crate there; `[dev-dependencies]` may list contract crates for tests. One exception: `examples/local-voice-cascade` keeps `inference-llama-server`, `inference-sherpa-onnx` and `inference-whispercpp`, because its `local` module chooses the recogniser per run and refuses a non-loopback server, which binding one adapter per role by name cannot express (`story:sdk-examples`) |
| T4 | every SDK application runs offline in tests with the same code path, a scripted stack and a fake clock | `inference::testing` drives the T1 example in `cargo test` with no device, network or weights |
| T5 | no layer is hidden: from every SDK handle the layer beneath is one method away | `session()`, `pipeline()` and `describe()` on `Agent`, `Chat`, `Transcriber` and `Voice`, `recipe.declaration()`, `Event::raw()` and `stack.adapter(Role)` exist and are tested |
| T6 | usage of every model call is on every handle without opt-in, and its cost wherever a price exists (Q5) | `usage()` on `Agent`, `Chat`, `Transcriber` and `Voice` equals the sum of that handle's `UsageRecorded` events |
| T7 | an error names its fix: the feature to enable, the variable to set, the file and key to change | every `inference::Error` variant's `Display` is snapshot-tested, and the six of §9 that the user can fix name the fix |

## 3. Five layers, one import

```text
L0  one call           inference::transcribe(path)   inference::chat(prompt)   inference::speak(text)   inference::talk()
L1  handles            Agent  Chat  Transcriber  Voice           (builders, async, one Event stream)
L2  recipes            Recipe::voice_cascade()  Recipe::native_voice()  Recipe::chat()  … → a planned pipeline + a session
L3  graph              inference::graph::Builder over inference::Payload (one payload, every payload trait)
L4  contracts          inference::{core, text, audio, stt, llm, tts, duplex, tools, pipeline, runtime, facade}
```

A layer uses only the layer under it. An application starts at the top and steps down one method at
a time when it needs to (T5); it never has to rewrite what it had to go one layer lower.

### 3.1 L0: one call

```rust
let text = inference::transcribe("meeting.wav").await?;
let reply = inference::chat("Summarise this in one line: ...").await?;
inference::speak("Done.").await?;          // the default speaker
inference::talk().await?;                  // a voice agent on the default microphone and speaker until Ctrl-C
```

Each resolves the stack (§4), opens what it needs, runs, closes, and returns. `talk` is T1 with no
tool and no output.

```rust
let text = inference::transcribe_with(&stack, "meeting.wav").await?;
let reply = inference::chat_with(&stack, "Summarise this in one line: ...").await?;
inference::speak_with(&stack, "Done.", host).await?;  // any stack, any AudioHost: a scripted one and the fake one in tests
inference::talk_with(&stack, host).await?;
let rows = inference::sessions().sessions_view();     // every SDK session, open or closed
```

The `_with` form of a call takes what the plain form defaults, in one order: the stack first, then
the plain form's own arguments, then the `AudioHost` where the plain form uses the default one. It
uses the stack as given and resolves nothing more; the plain form is the `_with` form over
`Stack::from_env()` and, for `speak` and `talk`, the default host. `Agent::voice_with(stack)` follows
the same rule. The handles do too: `Chat::new()`, `Transcriber::new()` and `Voice::new()` are
`Chat::new_with(&stack)`, `Transcriber::new_with(&stack)` and `Voice::new_with(&stack)` over
`Stack::from_env()`. `speak_with` and `talk_with` take an `AudioHost` and sit behind the feature
`audio-host` (no cpal); `speak` and `talk` open the default `CpalHost` and sit behind
`audio-device`, which implies `audio-host` and adds cpal. `transcribe`, `chat` and their `_with`
forms need neither. `inference::sessions()` is the process-wide `LiveSessions`
(`crates/inference-facade/src/live.rs:532`) every SDK session opens on, so an application can list
them. All of them are public.

### 3.2 L1: handles

```rust
use inference::prelude::*;

/// The local time, as HH:MM.
#[inference::tool]
async fn local_time() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

#[inference::main]
async fn main(host: impl AudioHost, stack: Stack) {
    let agent = Agent::voice_with(stack)
        .instructions("Answer briefly.")
        .tool(local_time)
        .start()
        .await?;
    agent.run_on_devices_with(host).await?;
    println!("{}", agent.usage());
}
```

That is T1: fifteen counted lines, the non-blank, non-comment lines of the text above in rustfmt's
layout (the `use` line, four of the tool, ten of `main` with its attribute). Without the tool, and
with the agent built in one line (`Agent::voice_with(stack).start().await?`), seven.

`#[inference::main]` turns that function into the program. The body declares no return type; the
macro makes it return `inference::Result<()>` and appends `Ok(())` (under
`#[allow(unreachable_code)]`), so `?` works inside it. It emits:

- `pub async fn run(host: impl AudioHost, stack: Stack) -> inference::Result<()>` holding the body,
  which tests call with the fake host and a scripted stack (T2, T4);
- a `fn main() -> ExitCode` that starts a multi-thread Tokio runtime (a hosted adapter may block its
  calling thread while it connects, as the AWS binding does in its own pool), calls `run` with the
  default audio host (`CpalHost`) and `Stack::from_env()`, and hands the result to
  `inference::__private::exit`, which prints `error: {e}` and each `source()` through `Display` (T7)
  and returns `ExitCode::FAILURE`.

`agent.run_on_devices_with(host)` returns when the session ends or on Ctrl-C. `agent.usage()`
displays the token counts and, when the stack names a price document, the cost; without one it
shows `cost: unknown (no prices)`, so printing it never fails (§7). `Agent::voice_with(stack)` is `Agent::voice().stack(stack)`;
`Agent::voice()` alone resolves the stack itself (§4). `Agent::voice()` asks the stack for a speech role; with
one bound it plans `Recipe::native_voice`, otherwise `Recipe::voice_cascade` over the stack's STT,
LLM and TTS (T2). The choice is reported by `agent.describe()` and never made silently across a
profile boundary (§4.3).

An application that wants the events instead of the devices reads the stream `agent.events()`
returns. The first call gets every event since the session opened; a later call sees what comes
after it. The agent itself is not a stream, so one task can read while another sends audio:

```rust
let agent = Agent::voice().start().await?;
let mut events = agent.events();
agent.send_audio(frame).await?;
while let Some(event) = events.next().await {
    match event? {
        Event::Heard { text, final_ } => …,
        Event::Said { text, .. } => …,
        Event::Audio(frame) => speaker.play(frame),
        Event::ToolCalled { name, .. } => …,
        Event::Usage(record) => …,
        Event::Interrupted { .. } => …,
        Event::Closed { .. } => break,
        _ => {}                             // Event is #[non_exhaustive]
    }
}
```

`Event` is the SDK's vocabulary: fewer variants than `SessionEvent` and the duplex events together,
each carrying plain values. This is the one list of its twelve variants: the seven above (`Heard`,
`Said`, `Audio`, `ToolCalled`, `Usage`, `Interrupted`, `Closed`) and `Started` (carrying the instant of
the agent's `inference::Clock`), `Transcript`, `Error`, `Lagged(n)` (a subscriber fell more than
1024 events behind the agent's broadcast channel and missed `n`) and `ConfirmationRequested {
call_id, name, arguments }` (a tool that requires confirmation is held until `agent.confirm(id)` or
`agent.reject(id)`; `close` rejects every held call first). `ToolCalled` also carries the
provider's `speech_call_id`; it comes from the session's `ToolCallProposed` and precedes the call's
`ConfirmationRequested`. `Event` is `Clone`. `event.raw()` returns the underlying `SessionEvent` or duplex event (T5).
Every stream ends after `Event::Closed`. The first `events()` call yields every buffered event even
when made after the end, then ends; a later call after the end ends at once. `Interrupted` comes
from `agent.interrupt()` or, with the session's `auto_interrupt` on, from a barge-in: the user
starts speaking while a reply is in progress (from the reply's `GenerationStarted` to its
`GenerationCompleted`) or its audio is still pending at the speaker. A barge-in cuts the speaker at
once, drops the cut reply's later frames and calls the session's `interrupt()`; with
`auto_interrupt` off it does nothing and the application decides. A frame belongs to the reply of
the last `GenerationStarted` before it, and a cut, the barge-in's or `agent.interrupt()`'s, ends
that reply, so a second `SpeechStarted` yields no second `Interrupted`. `auto_interrupt` is an agent
setting that survives every switch and controls barge-in on both topologies: a native session's
through `SpeechStarted`, a cascade's through `TurnOpened`. A cascade reports no `SpeechStarted`;
its speaker sink cuts queued and in-flight frames when the next turn opens, counts the
interruption and yields one `Interrupted`, as the reference cascade's acknowledging sink does.
Off means a `TurnOpened` cuts nothing. A failed response yields
`Event::Error`, never `Interrupted`. All of a native session's events leave the duplex node on one
port, so the agent yields them in the order the model produced them. A cascade promises no such
order: its events leave several nodes on several egresses, and the speech-to-text node sends a
transcript downstream before its exposed egress
(`crates/inference-testkit/src/stt/node.rs:443-448`), so a reader may see `Said` before the
`Heard` it answers. Later milestone: sequence-stamped events, so cascade order becomes causal. The send-order swap is not enough on its own (`live.rs:976-993`).
A cascade has no audio egress either: its speaker sink node plays through the agent's own audio
route, the route native audio takes, so a cascade reply reaches the speaker the same way. While
`run_on_devices` runs, audio goes to the speaker only, never onto the stream; before and after, it
is `Event::Audio`, and then the caller owns playback and cutting: `Event::Interrupted` tells it
when to cut. So during a device run only more than 1024 non-audio events can lag a reader
that waits until the end.
`agent.close().await` drains: every event received before it arrives before `Event::Closed` (`LiveSession::close`, `crates/inference-facade/src/live.rs:881`, and the session's drain at
`docs/design/inference.md:718`).

| handle | built from | does |
|---|---|---|
| `Agent` | `Agent::voice()`, `Agent::text()` | a session over a recipe: send text or audio, receive `Event`, interrupt, switch pipeline, close |
| `Chat` | `Chat::new()`, `Chat::new_with(&stack)` | text in, text or a delta stream out, history kept, tools allowed |
| `Transcriber` | `Transcriber::new()`, `Transcriber::new_with(&stack)` | audio frames or a file in, committed spans out |
| `Voice` | `Voice::new()`, `Voice::new_with(&stack)` | text in, audio frames out, played or returned |

Every handle carries `usage()` (T6), `describe()`, and the escape methods of §3.5.

### 3.3 L2: recipes

A recipe is a named, parameterised topology that declares, validates and plans a pipeline over a
stack. It is what `examples/local-voice-cascade/src/lib.rs:515-690` is, made reusable.

```rust
let recipe = Recipe::voice_cascade()
    .vad(VadConfig { .. })
    .with_node("tts", my_synthesiser_node)   // replace one node's implementation
    .expose("assistant_text");
let agent = Agent::from_recipe(recipe).start().await?;
```

| recipe | mode | roles it binds | topology |
|---|---|---|---|
| `chat` | `turn_based` | llm | text in, LLM, text out (topology A, `docs/design/inference.md:748`) |
| `transcribe` | `batch` or `continuous` | stt | audio in, STT, committed text out |
| `speak` | `batch` | tts | text in, TTS, audio out |
| `voice_cascade` | `turn_based` | stt, llm, tts | topology B (`docs/design/inference.md:758`), as `local-voice-cascade` declares it |
| `native_voice` | `duplex` | speech | audio in, the native model, audio and text out (topology D, line 789); the model's tool calls run through the runtime's `ToolAuthority` and `ToolExecutorNode` and return to the model by its own call id; a call to a tool the recipe was not given is denied by the duplex node, one past the feedback bound (4) or refused confirmation is answered as rejected, and a call that requires confirmation waits for the agent's `confirm` or `reject` |

A recipe is data plus a declare function; its declaration is reachable (`recipe.declaration()`),
and an application may write its own recipe with the L3 builder and register it by name:
`Recipe::custom(name: &str, mode: InteractionMode, declare: impl Fn(&mut inference::graph::Builder<'_, '_>) -> Result<(), Error> + Send + Sync + 'static) -> Recipe`,
`Recipe::role(self, node_name: &str, role: Role) -> Recipe` for each node that needs a binding,
`Recipe::register(self) -> Result<(), Error>`, which refuses a name already held, and
`Recipe::named(name: &str) -> Option<Recipe>`, which finds a built-in or registered recipe.
The recipe table lives in the `inference` crate, process-wide, so `named` needs no handle; it is
append-only and keyed by name, with no removal, so a test registers under a name of its own.
`Recipe::declare_into(&self, declaration: &mut Declaration, catalog: &dyn Catalog, profile_id: ProfileId, adapters: &[(Role, AdapterId)], requires: &[(Role, &[Capability])]) -> Result<PipelineId, Error>`
declares a recipe into a caller-owned declaration over the caller's catalog and profile, which the
caller then validates and plans; a stack is such a catalog, and `recipe.declaration()` is the
recipe's unbound topology under a built-in `offline_strict` profile.

Every exposure of the five recipes, each ingress and egress, buffers `inference::recipe::EXPOSURE_CAPACITY`
events, 64 by default, under `OverloadPolicy::Pause`. `Recipe::exposure_capacity(n)` sets it for
one recipe. It trades latency against buffering: a smaller value bounds how far the application may
fall behind the model before the model's output pauses, and how much is queued behind an
interrupt, at the cost of more pauses when the application reads in bursts.

### 3.4 L3: one payload

`inference::Payload` is one enum carrying every port modality the workspace's nodes speak, and it
implements every payload trait in the workspace (`AudioPayload`, `TurnPayload`, `GatePayload`,
`ToolPayload`, `SttPayload`, `ModelPayload`, `TtsPayload`, `SpeechPayload`, `DuplexPayload`,
`PlaybackPayload`, `SessionPayload`).
`VoicePayload` (`examples/local-voice-cascade/src/lib.rs:182`) becomes unnecessary (G4).

It lives in the `inference` crate (`crates/inference/src/payload.rs`), not in `inference-pipeline`:
`SessionPayload` is in `inference-facade` (`crates/inference-facade/src/events.rs:124`), which
depends on the pipeline, and `PlaybackPayload` is in `inference-audio-device`
(`adapters/inference-audio-device/src/acknowledging.rs:136`) behind its `pipeline` feature, so a
payload in the pipeline could implement neither without a crate cycle.

The graph builder in `inference::graph` is `inference-pipeline`'s typed builder with that payload
fixed. The pipeline defaults the port traits per modality (`crates/inference-pipeline/src/builder.rs`),
so the cascade's port block shrinks to its edges.

### 3.5 Escape methods

| from | method | returns |
|---|---|---|
| `Agent` | `session()` | `&inference_facade::live::LiveSession<Payload>`, shared with the agent's pump |
| `Agent` | `pipeline()` | `&PlannedPipeline` |
| `Agent`, its builder | `describe()` | `inference::Description`: the recipe chosen, each role's binding and its source, the `inference_facade::describe` output (nodes, bindings, effective capabilities, downgrades) and the `SessionConfig` the agent opens with; on the builder it resolves and plans but opens no session |
| `Recipe` | `declaration()` | the `inference_pipeline::Declaration` before planning, its nodes unbound |
| `Event` | `raw()` | the `SessionEvent` or duplex event it was made from |
| `Stack` | `adapter(Role)` | the bound adapter's descriptor and capability record |

## 4. Stacks: which adapter does what

### 4.1 The stack

A stack binds each role (`stt`, `llm`, `tts`, `speech`) to one adapter and names the deployment
profile it runs under. It is the SDK's only configuration noun (`inference.sdk.Stack`). With no
`profile` key the profile is `offline_strict`, so a hosted adapter runs only under a stack that
names `hybrid_opt_in`. The profile has the sources and order of §4.2: `Stack::profile(..)`, then
`INFERENCE_PROFILE`, then the files.

```toml
# inference.toml
profile = "offline_strict"

[stt]
adapter = "sherpa-onnx"
dir = "${INFERENCE_SHERPA_ONNX_STT_DIR}"

[llm]
adapter = "llama-server"
url = "http://127.0.0.1:8080"

[tts]
adapter = "mock"
```

```toml
# inference.toml for a hosted native model
profile = "hybrid_opt_in"

[speech]
adapter = "openai"
```

### 4.2 Resolution order

First found wins, per role: the builder (`Agent::voice().llm(adapter)`), then
`INFERENCE_<ROLE>_ADAPTER` and its keys as `INFERENCE_<ROLE>_<KEY>`, then the file `INFERENCE_STACK`
names, then `./inference.toml`, then `$XDG_CONFIG_HOME/inference/inference.toml`. `Stack::resolve()`
returns where each binding came from, and `agent.describe()` prints it. A role's keys (`dir`, `url`,
…), from the environment or a file, reach the adapter's registered constructor after `${VAR}`
expansion.

Every builder method and the scripted stack bind through one seam, `Stack::bind(Binding)`. It
takes no role, because every binding carries its own: `Binding::named(Role, adapter, keys)`, or a
typed adapter instance built by one constructor per role (`Binding::stt(impl Recogniser)`,
`Binding::llm(impl Into<Generator>)`, `Binding::tts(impl Synthesiser)`,
`Binding::speech(impl NativeSpeechModel)`; no `dyn Any`), recorded with the source `builder`.
The role is in the type: each typed constructor returns `Bound<role::Stt>`, `Bound<role::Llm>` and
so on, which the agent builder's `stt`, `llm`, `tts` and `speech` take, so
`Agent::voice().llm(Binding::tts(..))` does not compile. `Binding::named` returns a runtime-roled
`Binding`, which only `Stack::bind(impl Into<Binding>)` takes.
`Binding::model(self, name)` sets the role key `model` (`[<role>] model`,
`INFERENCE_<ROLE>_MODEL`): the serving model name the role's prices are looked up by (§7), its
default the provider model string the adapter is configured with, where it has one. The testkit mocks are such instances, reached behind the
feature `testing`. `Stack::new()` is an empty stack.
`stack.resolve_with(&Env)` reads the other sources from that `Env` (variables, working directory,
config home) and fills only the roles still unbound, so a role bound with `bind` keeps its builder
binding whatever the environment says, and the order above holds. `Stack::from_env()` is
`Stack::new().resolve_with(&Env::process())`, and tests pass an `Env` of their own, built as
`Env::new().var("INFERENCE_LLM_ADAPTER", "mock").working_dir(dir).config_home(home)`.

### 4.3 What a stack never does

- It never binds a hosted adapter under a profile other than `hybrid_opt_in`: the stack declares its
  profile through `inference.policy.DeclareProfile`, and planning refuses the adapter
  (`inference.graph.PlanPipeline` `locality-denied`, `ess/domains/policy.yaml` header).
- It never falls back from a failed adapter to another one. A fallback is a recipe the application
  chose (`docs/design/inference.md:900`), never a default.
- It never reads a credential itself. Hosted adapters read theirs as they do today (environment, or
  the `secrets` library behind the adapter's feature).

### 4.4 Registry and features

The `inference` crate registers a constructor for each adapter under its name when its SDK feature
is on (`inference = { features = ["sherpa-onnx", "llama-server", "openai"] }`), one file per
adapter in `crates/inference/src/registry/`. The registrations stay out of the adapter crates:
`inference` depends on them, so a registration inside an adapter would need `inference` and close
a cycle. A stack naming an adapter
whose feature is off fails with `Error::AdapterNotCompiled { name, feature }`, whose message is the
`Cargo.toml` line to add (T7). The registry is open: an application registers its own adapter by
name and a stack file can then select it. The adapter registry is process-wide and append-only,
keyed by adapter name alone (one descriptor per name, listing every role it serves), with no
removal; `Registry::register(AdapterDescriptor) -> Result<(), Error>` refuses a name already
held, a built-in's (reserved whatever the features) or an earlier registration's, with
`Error::Layer` naming it. The recipe
registry follows the same rules (§3.3).

A speech adapter's registration also carries its default `SessionConfig`: voice, formats, turn
detection, `auto_response`, `auto_interrupt` and idle limit. The values are today's
`examples/realtime-voice/src/lib.rs:106-160`: OpenAI Realtime at 24 kHz both ways with
`auto_interrupt` off, GPT-Live with semantic turn detection and `auto_interrupt` on, Nova Sonic at
16 kHz in and 24 kHz out. Each duplex provider crate exports its default as
`default_session_config()` (`inference-openai`'s `duplex` and `live_duplex`, `inference-aws`'s
`duplex`), and the registry uses that export, so a test can pass the same value; this is the one
change the SDK makes inside the adapter crates. The stack's `voice` key (`[speech] voice`,
`INFERENCE_SPEECH_VOICE`) overrides the voice; the builder's `session_config(..)` replaces the
whole config, and the builder's `voice(..)` then overrides the voice. The builder's instructions and
tools fill the rest, and `describe()` shows the merged config.

Resolving a stack does no I/O. A registration builds no client: Nova Sonic's is bound through
`NovaSonicSpeech::deferred(model_id)` in `inference-aws`'s `duplex`, whose connector loads the AWS
configuration when a session opens, so an AWS-bound stack resolves and describes itself with no
credentials and no network.

## 5. Tools

`#[inference::tool]` on an `async fn` whose arguments are `Deserialize` and whose return is
`Serialize` (or `Result<_, E: Display>`) produces an `inference_tools::Tool` (name from the function,
description from its doc comment, JSON schema from the arguments) and a `ToolExecutor`. The
proc-macro lives in `inference-macros`; it replaces the function by a unit struct of the same name
that converts into the `(Tool, Box<dyn ToolExecutor>)` pair `Agent::tool` takes. Tool authority
and effects stay as the workspace enforces them (O3 of `vision:inference`): a macro tool is
`read_only` unless it says `#[inference::tool(effect = "idempotent_write")]` or
`"non_idempotent_write"`, the classes of `SideEffectClass` (`crates/inference-tools/src/lib.rs:67`),
and a tool that is not read-only is refused on provisional input
(`crates/inference-tools/src/call.rs:198`, `ProvisionalInputBarrier`). `#[inference::tool(confirm)]`
sets `requires_confirmation`, and `crates/inference-pipeline/src/nodes/tool_authority.rs` holds the
call until the host confirms it.

## 6. Testing (T4)

`inference::testing` is the testkit behind one door:

```rust
/// The time of day, fixed for the test.
#[inference::tool]
async fn time_of_day() -> String {
    "10:30".to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn answers_the_time() {
    let stack = Stack::scripted()
        .hears("what time is it")
        .calls("time_of_day", json!({}))
        .says("It is 10:30.");
    let agent = Agent::voice().stack(stack).tool(time_of_day).start().await.unwrap();
    let mut events = agent.events();
    agent.send_text("what time is it").await.unwrap();
    assert_events!(events, [ToolCalled { name: "time_of_day", .. }, Said { text: "It is 10:30.", .. }]);
}
```

The tool is defined in the test and returns a fixed string, so the test needs no clock crate.
`assert_events!` awaits: it reads the next events from the stream it is given, one per pattern, so it
is used inside an `async` test and the `.await` is in its expansion, not at the call site. It
checks each event's variant and compares each named field with `==`, so a `String` field such as
`text` matches a `&str` literal through std's `impl PartialEq<&str> for String`; a mismatch names
the field.

`Stack::scripted().speech(MockSpeechModel::native(turns))` scripts the duplex mock for the `speech`
role, so a native or a mixed stack is scripted as a cascade is; `.closes()` on the mock ends the
session once its script is spent and nothing is pending, so a device run over it returns on its
own. A node cannot end its run, so the duplex node reports the close as
`SessionEvent::SpeechSessionClosed`, and the agent closes its session on it. The scripted stack binds the testkit mocks (`inference-testkit` features `stt`, `llm`, `tts`,
`duplex`) to the same recipes; the fake audio host (`FakeHost`, reached under `testing` through
`audio-host`, with no cpal) stands in for devices: its output accepts writes with no `pull`, and
`FakeHost::played()` returns every frame written to it, with a `Cleared` entry where playback was
stopped at a barge-in (`played_frames()` holds the frames only), and `FakeHost::hold_sink()` holds
writes until its guard drops, so a test can show a run waits for its last reply. The mocks' reply audio is never silent:
each frame's samples encode the mock and the reply and frame index, and
`inference::testing::mark::decode` reads them back, so a test asserts which mock spoke and that a
reply played to its last frame. `FakeClock`
implements the agent's `inference::Clock`. Nothing in the SDK branches
on being under test: the recipe, the session and the event mapping are the production ones.

## 7. Usage and cost (T6)

Every handle sums the `UsageRecorded` events of its runs (`crates/inference-facade/src/usage.rs`),
for cascades and native models alike once G2 is closed. `usage().cost()` prices them through the
facade's `cost` feature (`b10x-llm-cost`) with the price document the stack names
(`prices = "prices/openai.json"`) or the application gives it with `Stack::prices(book)`; with none it returns
`Error::NoPrices`, never zero.

The facade prices only what `llm-cost` has a unit for: audio tokens, audio milliseconds, characters
and requests get a `price_unknown` line (`crates/inference-facade/src/cost.rs:24-27`), never zero.
A native speech session's cost is therefore a lower bound with its audio lines unknown, while
`examples/realtime-voice/src/usage.rs` prices audio tokens with its own table. Until `llm-cost`
prices audio (Q5), `realtime-voice` keeps its whole tally, its text-token pricing included, because
the SDK's cost would leave its audio tokens unknown. The SDK's own text-token pricing is checked by
the fixture of `story:sdk-usage-cost`.

A price document has no adapter field: its model prices are keyed by price id and serving model
id. The join is on a name the user can write, never on the adapter's `ModelId`, a generated UUID
(`crates/inference-core/src/model.rs:23`, `:170`). Each binding's serving model name is its role
key `model` (§4.2), by default the provider model string the adapter is configured with (OpenAI
Realtime `gpt-realtime-2.1`, GPT-Live `gpt-live-1`, Nova Sonic its model id, llama-server its
served model). For each binding with a name, the stack looks up the entry whose
`serving_model_id` is that name and binds that adapter's `PriceBinding` from it
(`crates/inference-facade/src/cost.rs:76-118`), and the role's node runs inside the facade's
`Serving` wrapper, which stamps the matched entry's `upstream_model` on each usage record that
names none, so an entry whose `upstream_model` differs from its `serving_model_id` still prices. A binding
with no name (sherpa-onnx, whispercpp, a value binding without `.model(..)`) or with no entry
stays unpriced, and its records' cost shows unknown.

`Usage` implements `Display`: `{input} input tokens, {output} output tokens, cost: {cost}`, the
cost when a price document is named and otherwise `cost: unknown (no prices)`. A count no record
reported, or a sum with an unknown part, shows as `unknown`, never 0. Displaying usage never fails; only `cost()` returns
`Error::NoPrices`.

`Cost` has public fields: `currency`, `lines` (each `CostLine { kind, quantity, amount, unknown }`,
a priced line with an `amount`, an unpriced one with `unknown: price_unknown`) and `total`
(`CostTotal { known_subtotal, unknown_lines, complete_total }`, `complete_total` present only when
no line is unknown). It displays as `0.000864 USD` when complete and as
`at least 0.000864 USD, 2 unknown` otherwise; a priced usage line reads
`12 input tokens, 34 output tokens, cost: 0.000864 USD`.

## 8. Runtime and threading

The contracts stay runtime-neutral (`docs/design/inference.md:82`). The SDK is where a runtime is
chosen: Tokio, which every hosted adapter already uses (`adapters/inference-openai/Cargo.toml:30`,
`adapters/inference-aws/Cargo.toml:21`). Handles are `Send`, asserted at compile time for `Agent`,
`Chat`, `Transcriber` and `Voice`; `Agent` is also `Sync`. It is not a `Stream`: `agent.events()`
returns one (`Events`, over `tokio_stream::wrappers::BroadcastStream`), so reading needs no `&mut` of the agent.
`Agent`'s methods take `&self`, except `switch`. It holds its session with no lock: the SDK changes the facade so `LiveSession::receive` and
`close` take `&self` (today `&mut self`, `crates/inference-facade/src/live.rs:881`, `:967`), its
receive state behind a `std::sync::Mutex` and its ingress list cloned out before any await. One
pump task owns `receive`, because a session has one waker slot (`live.rs:431-435`), and fans the
events out through a `tokio::sync::broadcast` channel of capacity 1024. So a handle shared through
`Arc` can call `send_audio` or `close` while `run_on_devices` runs, and a slow reader gets
`Event::Lagged` instead of stalling the session. What the pump, a spawned close and `Drop` share
sits in one `Arc<Inner>`: the session, the lifecycle state (`Open`, `Switching`, `Closing`,
`Closed`, moved by compare-and-swap), the tool authority's handle and the
confirmations already announced, the speaker slot, and the event `Sender`. `switch` keeps
`&mut self` on both, because it reassigns the session's fields
(`crates/inference-facade/src/switch.rs:120-129`): the agent moves `Open` to `Switching`, aborts its pump, awaits it, and takes
the session through `Arc::get_mut`; once a close has started it refuses with `Error::Closed`. Every
failed switch leaves either a pump running on the old session or an agent that has yielded
`Closed`. The agent spawns the `SessionDriver` that `open` and `switch` return
(`crates/inference-facade/src/live.rs:1241`) and keeps its handle; the first of a failed run, a failed model session, the driver ending, the session's close and the
end of its events closes the agent, with one `Event::Error` for a failure and `Closed` once. A switch raises the session's epoch: the old run's events and
its run end arrive before the new run's (`switch.rs:104-115`), so the new pump drops every `SpeechStarted`, close and
run end up to the old run's `RunCompleted`, and the old model's close cannot close the new session.
The agent's own settings (`auto_interrupt`, the event channel, the speaker) survive a switch; the
model and session configuration come from the new recipe. `agent.closed()` resolves once the pump has dropped the `Sender`. The
pump never awaits a close: on the model's close it spawns one and keeps receiving, so the egress it
drains never blocks the stop. When the session ends the pump drops the `Sender`, so every reader
ends. Dropping an unclosed agent cancels its session and aborts the pump. Closing is idempotent,
whichever of the application and the model closes first. `#[inference::main]`
starts a multi-thread runtime (§3.2). A blocking wrapper (`inference::blocking`) is a later milestone.

## 9. Errors (T7)

One `inference::Error`, `#[non_exhaustive]`, with `kind() -> ErrorKind` and the underlying error
as `source()`. `inference::ErrorKind`, also `#[non_exhaustive]`, has one value per variant, named
as it. `Layer` holds a message `String` and an optional boxed `std::error::Error + Send + Sync`
source, which `source()` returns.
Variants map one to one onto what the user can change: `AdapterNotCompiled`, `RoleUnbound`,
`ProfileDenies`, `StackFile`, `Device`, `Provider`, `NoPrices`, `Closed`, and `Layer` for an error
of the layer beneath that the SDK does not classify. The six the user can fix name the fix in their
`Display`: `AdapterNotCompiled` the line `features = ["<feature>"]`, `RoleUnbound`
`INFERENCE_<ROLE>_ADAPTER` and the `[<role>]` table, `ProfileDenies` the role, the adapter and the locality it
runs at, then the text `profile = "hybrid_opt_in"` with every source that sets it (a stack file,
`INFERENCE_PROFILE`, `Stack::profile`), `StackFile`
the file and key, `Device` the selector and the device names the host reports, `NoPrices` the key
`prices`. `Provider`, `Closed` and `Layer` carry the message of what failed beneath.

## 10. Prerequisites the SDK cannot build on until they exist

| gap | the story that closes it |
|---|---|
| G1 model nodes live in the testkit | `story:sdk-model-nodes` moves `SttNode`, `ModelNode`, `AgentNode`, `SpeechNode`, `TtsNode` into `inference-pipeline::nodes`, with the `StreamEvent`, `EventLog` and `RunLog` they emit into, the testkit re-exporting them |
| G2 no node runs a native speech model | `story:sdk-duplex-node`: a `DuplexNode` over `NativeSpeechModel`, so a native model runs under the runtime and a session |
| G4 no shared payload | `story:sdk-payload` |

## 11. Out of scope

- A server, a CLI or a network transport (`docs/design/inference.md:722-731`): this repository ships
  libraries (`AGENTS.md`, Rules). A transport crate is a separate decision.
- Bindings for other languages: running code here is Rust.
- Automatic provider fallback, retries across providers, and routing by price.
- Structured output (`chat.ask::<T>()`): depends on what `b10x-llm-core` 0.8.3 exposes; not checked.

## 12. Open questions

| id | question | default taken in this draft |
|---|---|---|
| Q1 | the crate name `inference` on crates.io is not checked; publishing is out of scope either way | use `inference` as the workspace crate name; publishing decided at release |
| Q2 | where the production model nodes live: `inference-pipeline::nodes` or a new `inference-nodes` | `inference-pipeline::nodes`, beside `vad`, `turn_policy` and `commit_gate` |
| Q3 | JSON schema for `#[inference::tool]` arguments: `schemars` or a derive of our own | `schemars` |
| Q4 | `Event` variant set | the list in §3.2; fixed by `story:sdk-agent` |
| Q5 | `llm-cost` has no unit for audio tokens, so the facade prices them `price_unknown` (`crates/inference-facade/src/cost.rs:24-27`) | audio lines stay `price_unknown`, never zero; `realtime-voice` keeps its own tally until `llm-cost` prices audio |
