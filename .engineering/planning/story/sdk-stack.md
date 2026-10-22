---
format: aep.planning-md/3
id: story:sdk-stack
kind: story
status: draft
title: A stack binds each role to one adapter from builder, environment or TOML, resolved in a fixed order
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-crate
- depends_on: story:duplex-late-usage-counted
scope:
- confidence: cited
  path: Taskfile.yml
- confidence: cited
  path: adapters/inference-aws/src/duplex.rs
- confidence: cited
  path: adapters/inference-openai/src/duplex.rs
- confidence: cited
  path: adapters/inference-openai/src/live_duplex.rs
- confidence: inferred
  path: crates/inference-testkit/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/tests/conform.rs
- confidence: inferred
  path: crates/inference-testkit/tests/target/mod.rs
- confidence: inferred
  path: crates/inference-testkit/tests/target/sdk.rs
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
  path: crates/inference/tests/stack.rs
- confidence: inferred
  path: ess/domains/sdk.yaml
revision: 23
---
## Context

`docs/design/sdk.md` §4: a stack binds each role to one adapter under one deployment profile, read
from the builder, the environment or a TOML file, first found wins. Today each example has one
configuration function per provider and a `match` in the binary
(`examples/realtime-voice/src/lib.rs:110`, `:129`, `:147`; `examples/realtime-voice/examples/talk.rs:82`).
Targets T2 (configuration side), T5 (`Stack::adapter(Role)`) and T7 (`AdapterNotCompiled` from a
real resolve). Serves O1.

## Domain relations

`inference.sdk.Stack` owns many `inference.sdk.StackBinding`; each binding references one
`inference.model.Adapter`; the stack references one `inference.policy.DeploymentProfile`
(`ess/domains/sdk.yaml`). This story declares the command `inference.sdk.ResolveStack` there, with
the outcomes `resolved`, `role-bound-twice`, `locality-denied` (through `inference.policy`) and
`unknown-adapter` (the repository's refusal naming, as `DeclareAdapter/outcome/unknown-model`).

## Planned scope

- `ess/domains/sdk.yaml`: `ResolveStack` and its outcomes; `ResolveStack` removed from the UNMAPPED
  comment (`:5-7`).
- `crates/inference/src/stack.rs`:
  - `Stack`, `Stack::new()` (an empty stack), `Stack::adapter(Role)`, the TOML and environment
    readers;
  - `pub enum Role { Stt, Llm, Tts, Speech }` (R194), created by this story in `stack.rs` and
    reached as `inference::Role` through `lib.rs`'s glob. Its variants are those of the ESS enum
    `inference.sdk.Role`, `[stt, llm, tts, speech]` (`ess/domains/sdk.yaml:41-43`), and their
    lowercase names are the `<role>` of `INFERENCE_<ROLE>_*` and of a stack file's `[<role>]`
    table. It derives `Clone, Copy, Debug, PartialEq, Eq, Hash`;
  - what `Recipe::declare_into` (story:sdk-recipes) takes, built from a resolved stack (R197):
    `impl inference_pipeline::Catalog for Stack`, whose `profile(id)` returns the stack's
    `DeploymentProfile` when `id` is its id and whose `adapter(id)` returns the `Adapter` of the
    bound role carrying `id` (`crates/inference-pipeline/src/declaration.rs:27-33`);
    `Stack::profile_id(&self) -> ProfileId`, the id of the profile the stack declared from its
    `profile` key through `DeploymentProfile::declare` (`crates/inference-core/src/policy.rs:220`);
    and `Stack::adapter_ids(&self) -> Vec<(Role, AdapterId)>`, one pair per bound role, in the order
    `Stt`, `Llm`, `Tts`, `Speech`;
  - the `profile` key (R201, R204). It has the sources and the order of the role keys: the builder
    method `Stack::profile(self, ProfileKind) -> Stack`, then the variable `INFERENCE_PROFILE`
    (read from the same `Env`), then the top-level `profile = "<kind>"` (`docs/design/sdk.md`
    §4.1) of the file `INFERENCE_STACK` names, of `./inference.toml`, and of
    `$XDG_CONFIG_HOME/inference/inference.toml`, first found wins. `Stack::profile_source(&self)
    -> Option<BindingSource>` records where it came from, with the `BindingSource` values a
    binding uses, and `None` when the default applies. The value names the `ProfileKind`, and the stack declares that kind with its table egress
    (`ProfileKind::egress`, `crates/inference-core/src/policy.rs:50-56`), admitting every
    `Locality::ALL` member (`model.rs:115`) the table row allows (`TableRule::broken_by`,
    `policy.rs:118-128`). With no `profile` key
    in any source, the stack declares `offline_strict`: `ProfileKind::OfflineStrict`,
    `Egress::Denied`, admitting `Locality::SameProcess` and `Locality::LocalWorker`
    (`crates/inference-core/src/model.rs:106-111`), the localities of the local adapters and the
    testkit mocks. `Stack::profile_id()` then returns that profile's id. A hosted adapter needs
    `profile = "hybrid_opt_in"` named, as §4.3 requires; resolve itself does not refuse it, planning
    does (`Error::ProfileDenies` from story:sdk-recipes' instantiation, asserted by
    story:sdk-agent);
  - `Stack::bind(Binding)`, the one seam every builder binding goes through, recorded with
    `BindingSource::Builder`. A binding carries its role, so `bind` takes none:
    `Binding::named(Role, adapter, keys)` (a registered name and its role keys) or a typed adapter
    instance, `AdapterValue`, which has one variant per role. `Binding` has one value constructor
    per role, each taking that role's contract
    type: `Binding::stt(impl Recogniser)` (`crates/inference-stt/src/lib.rs:930`),
    `Binding::llm(impl Into<Generator>)` (`crates/inference-llm/src/lib.rs:707`; the llm crate
    declares no adapter trait, and an adapter such as `LlamaServer::generator`,
    `adapters/inference-llama-server/src/lib.rs:305`, yields a `Generator`),
    `Binding::tts(impl Synthesiser)` (`crates/inference-tts/src/lib.rs:1218`) and
    `Binding::speech(impl NativeSpeechModel)` (`crates/inference-duplex/src/contract.rs:338`),
    each bounded `Send + 'static`. Each boxes its value behind a crate-private per-role trait. A
    registered constructor returns the public `Binding` these constructors build (R219, decision
    D32 of `api-design:sdk`), so a value binding and a named binding resolve to one type; no
    `dyn Any`. The constructor fixes the role, so no value can be bound to another role.
  - The role is in the type (R104). `pub mod role` in `stack.rs` (re-exported at the crate root by
    `lib.rs`'s glob, so no `lib.rs` change) holds the zero-sized markers `role::Stt`, `role::Llm`,
    `role::Tts` and `role::Speech`, each implementing a sealed `role::Kind` with `const ROLE:
    Role`. The four value constructors return `Bound<role::X>`, a `Binding` whose role is that
    marker; `Bound<R>::model(self, name) -> Bound<R>`. `Binding::named(Role, adapter, keys)`
    returns the runtime-roled `Binding`, with `Binding::model(self, name) -> Binding`. `impl<R:
    role::Kind> From<Bound<R>> for Binding`, and `Stack::bind(self, binding: impl Into<Binding>)`
    takes either. So story:sdk-agent's `Agent::llm(Bound<role::Llm>)` refuses a `Bound<role::Tts>`
    and a runtime-roled `Binding` at compile time, while `Stack::bind` takes all of them. The
    testkit mocks are such instances (`MockRecogniser`, `MockGenerator`
    through its `From<MockGenerator> for Generator`, `crates/inference-testkit/src/llm.rs:353`,
    `MockSynthesiser`, `MockSpeechModel`); the SDK reaches them, and their scripts, only behind the
    feature `testing`. story:sdk-agent's builder methods, story:sdk-testing's `Stack::scripted()`
    and every SDK test bind through it;
  - `Env`, built by `Env::new()` (no variables, no working directory, no config home) and the
    builder methods `.var(name, value)`, `.working_dir(path)` and `.config_home(path)`, or taken
    from the process by `Env::process()`; and `stack.resolve_with(&Env)`, taking the
    stack by value, which reads every source but the builder from that `Env` only and fills only the
    roles still unbound, so a role bound with `bind` keeps its builder binding and the order is
    builder, environment, file, default; `stack.resolve()` is `stack.resolve_with(&Env::process())`,
    and `Stack::from_env()` (the stack an example's `main` hands to `run`) is
    `Stack::new().resolve_with(&Env::process())` (decision D17 of `api-design:sdk`);
  - role keys (`dir`, `url`, …) from `INFERENCE_<ROLE>_<KEY>` or the role's table in a stack file,
    after `${VAR}` expansion against the same `Env`, passed to the adapter's registered
    constructor (`docs/design/sdk.md` §4.1, §4.2). The `speech` role's key `voice`
    (`INFERENCE_SPEECH_VOICE`, or `voice` in the `[speech]` table) is read by the stack itself: it
    overrides the voice of the speech adapter's default `SessionConfig`, and
    `Stack::session_config()` returns that merged default (story:sdk-agent adds the builder's
    `voice`, `session_config`, instructions and tools on top);
  - the role key `model` (`INFERENCE_<ROLE>_MODEL`, or `model` in the role's table): the binding's
    model name, the string story:sdk-usage-cost matches against a price document's
    `serving_model_id`. `Stack::model(Role) -> Option<&str>` returns it. The `Adapter`'s own
    `model_id` is a generated UUID (`crates/inference-core/src/model.rs:23`, `:170`) and is never
    the price key. A registered constructor that configures a provider model passes the name to
    the provider; without the key, the binding's model name is the string the provider is
    configured with, read through the same `Env`:
    - `openai`: `Endpoint::new(model)` (`adapters/inference-openai/src/config.rs:31`); without the
      key, `MODEL_VARIABLE` (`INFERENCE_OPENAI_REALTIME_MODEL`) or `DEFAULT_MODEL`
      `"gpt-realtime-2.1"` (`config.rs:9`, `:12`, the rule of `Endpoint::from_env`, `:41-47`);
    - `openai-live`: `Endpoint::live(model)`; without the key, `LIVE_MODEL` `"gpt-live-1"`
      (`adapters/inference-openai/src/live.rs:90`, as `live_duplex.rs:292` uses it);
    - `aws`: `NovaSonic::new(client, model)` (`adapters/inference-aws/src/nova_sonic/client.rs:44`);
      without the key, `resolve_model_id` over `MODEL_ID_ENV` (`INFERENCE_NOVA_SONIC_MODEL`), so
      `DEFAULT_MODEL_ID` `"amazon.nova-2-sonic-v1:0"` (`nova_sonic/mod.rs:36-48`);
    - `llama-server`: the `served_model` of `Config::new(base_url, served_model)`
      (`adapters/inference-llama-server/src/lib.rs:154`), which the binding already needs;
    - `sherpa-onnx`, `whispercpp` and every value binding configure no provider model string, so
      their binding has a model name only when `model` or `Binding::model` gives one, and stays
      unpriced without it;
  - `default_session_config() -> SessionConfig`, the base speech default: voice `marin`, PCM16 at
    24 kHz in and out, `TurnDetection::ServerVad`, default `TurnTuning`, `auto_response` on,
    `auto_interrupt` off, idle limit 30 s, no instructions, no tools. A speech adapter bound by
    `Binding::speech(..)` opens with it (overridden as above), because a value carries no registry
    descriptor. `SessionConfig` has no `Default` (`crates/inference-duplex/src/contract.rs:34-46`).
- `crates/inference/src/registry/mod.rs`: the registry and `Registry::register` for application
  adapters. A registration is a descriptor: name, locality, the feature it sits behind, one
  constructor over role keys per role it serves, and, for the `speech` role, its default
  `SessionConfig`. The signature (R217, decision D32 of `api-design:sdk`):
  `Registry::register(descriptor: AdapterDescriptor) -> Result<(), Error>`, the descriptor built
  by `AdapterDescriptor::new(name: &str, locality: Locality)`, then
  `.role(role: Role, construct: impl Fn(&BTreeMap<String, String>) -> Result<Binding, Error> +
  Send + Sync + 'static)` once per role it serves, `.feature(&'static str)` and, for `speech`,
  `.session_config(SessionConfig)`, each returning the descriptor. The constructor returns the
  `Binding`, usually a typed `Bound<R>` converted with `.into()` (`From<Bound<R>> for Binding`). At resolve,
  `Error::Layer` refuses a returned binding of another role than the one it was given under, and
  a returned `Binding::named(..)`, so a registration cannot resolve to another name (R219).
  The table is keyed by adapter name alone: one descriptor per name, listing every role it
  serves, as `mock` serves all four. `Registry::register` adds to the one process-wide,
  append-only table the built-in registrations sit in, with no removal, so a test registers
  under a name no other test uses (R214); a name already held, a built-in's or an earlier
  registration's, is refused with `Error::Layer` naming it, as `Recipe::register` refuses (D31).
  The built-in names (`sherpa-onnx`, `whispercpp`, `llama-server`, `openai`, `openai-live`,
  `aws`, `mock`) are reserved whatever the features: `Registry::register` refuses each with
  `Error::Layer` naming it even when its feature is off and its registration not compiled in
  (R222).
  `registry/sherpa_onnx.rs`, `whispercpp.rs`, `llama_server.rs`, `openai.rs` (`openai` and
  `openai-live`), `aws.rs`: one registration each, behind its feature (decision D8 of
  `api-design:sdk`). The speech defaults move out of today's
  `examples/realtime-voice/src/lib.rs:106-160` (`openai_config`, `openai_live_config`,
  `nova_config`), without the example's instructions and tool, which the application gives
  through the builder. Each provider crate exports its own, beside the `capabilities()` that
  admits it, and the registry descriptor uses that export, so a test and the registry hold the
  same value:
  - `adapters/inference-openai/src/duplex.rs`: `pub fn default_session_config() -> SessionConfig`,
    the values of `:110-123` (voice `marin`, 24 kHz in and out, `ServerVad`, `auto_interrupt` off);
  - `adapters/inference-openai/src/live_duplex.rs`: the same with `TurnDetection::SemanticVad` and
    `auto_interrupt` on (`:129-142`);
  - `adapters/inference-aws/src/duplex.rs` (Nova Sonic): voice `matthew`, PCM16 at 16 kHz in and
    24 kHz out, `ServerVad`, `auto_interrupt` on (`:147-160`);
  - all three: `auto_response` on, idle limit 30 s, no instructions, no tools.
  Each sits in the module that is already behind the crate's `duplex` feature
  (`adapters/inference-openai/src/lib.rs:45-52`, `adapters/inference-aws/src/lib.rs:12-13`), and
  returns the `inference_duplex::SessionConfig` those modules already import (as `SpeechConfig` in
  `inference-openai`'s `duplex.rs:62` and `live_duplex.rs:75`, beside the crate's own wire
  `SessionConfig`; as `SessionConfig` in `inference-aws`'s `duplex.rs:69`). No other file under
  `adapters/` changes; the audio device is wired by story:sdk-agent's `devices.rs`, not the
  registry.
- Resolve does no I/O (R110). `NovaSonicSpeech::from_env` loads the AWS config when it is built
  (`adapters/inference-aws/src/duplex.rs:231-237`), so the `aws` registration does not call it.
  `adapters/inference-aws/src/duplex.rs` gains `pub struct Deferred` and `NovaSonicSpeech::<Deferred>::deferred(model_id: String) ->
  io::Result<Self>`, which is `with_connector` (`:247`) over that connector, which
  implements `Connector` (`:138-147`): `model_id` returns the string, and `connect` runs
  `aws_config::load_from_env()` into `sdk`, then `NovaSonic::new(Client::new(&sdk), model_id).connect(config)`
  (`nova_sonic/client.rs:44`, `duplex.rs:149`) inside the future it returns, so the client is built when a session
  opens. `registry/aws.rs` binds through `deferred`, with the model name of the binding. The adapter file
  is also story:duplex-late-usage-counted's, so this story depends on it (R109).
- `crates/inference/src/registry/mock.rs` (the stub story:sdk-crate declared, behind `testing`):
  the testkit mocks registered under the adapter name `mock`, for every role, the `speech` one with
  `default_session_config()`. A mock bound by value (`Binding::stt(<MockRecogniser>)`, …) runs its
  own script; bound by name it runs the testkit default.
  story:sdk-recipes,
  story:sdk-agent, story:sdk-one-call, story:sdk-usage-cost and story:sdk-testing use this
  registration and do not add one.
- `crates/inference/tests/stack.rs` (new). The feature-off test is gated
  `#[cfg(not(feature = "whispercpp"))]`, so `cargo test --workspace --all-features` compiles it
  out; it runs under `cargo test -p inference --no-default-features --test stack`.
- `Taskfile.yml`: that command as one more line of the `check` task's `cmds` (after
  `cargo test --workspace --all-features`), so the gate runs the feature-off test.
- Conformance: `crates/inference-testkit/tests/conform.rs` (a `SDK_SCENARIOS` array),
  `crates/inference-testkit/tests/target/mod.rs` (existing; one `mod sdk` and its dispatch arm) and
  `tests/target/sdk.rs` (new), and `crates/inference-testkit/Cargo.toml` (`inference` as a
  dev-dependency). The cycle follows a precedent in the workspace: the testkit depends on
  `inference-facade` optionally (`crates/inference-testkit/Cargo.toml:12`, feature `run`) and the
  facade dev-depends on the testkit (`crates/inference-facade/Cargo.toml:24-31`). Rule: the testkit
  takes `inference` only as a dev-dependency, and a test that uses both crates lives under
  `crates/inference-testkit/tests/`. `ess/SKIPPED.md` is unchanged.

## Acceptance

Conformance scenarios synthesized by `ess verify conform synthesize` from `ResolveStack` and
answered by the testkit target, each passing in
`cargo test -p inference-testkit --features run --test conform`:

- `inference.sdk.ResolveStack/outcome/resolved`
- `inference.sdk.ResolveStack/outcome/role-bound-twice`
- `inference.sdk.ResolveStack/outcome/locality-denied`
- `inference.sdk.ResolveStack/outcome/unknown-adapter`

The sixteen arrays present at the story's start (456 ids on main at `dc8e13e`) all still pass;
`SDK_SCENARIOS` is not counted in that number.

And Rust tests in `crates/inference/tests/stack.rs`. Every resolution test builds an `Env` with `Env::new()` (its
variables, a `tempfile` working directory and config home) and calls `resolve_with` on a `Stack::new()` or a bound stack; none
reads or sets the process environment, so the tests can run in parallel.

- Resolution order, for the role `llm`: a test sets all five sources (`Stack::bind`,
  `INFERENCE_LLM_ADAPTER`, the file `INFERENCE_STACK` names, `./inference.toml`,
  `$XDG_CONFIG_HOME/inference/inference.toml`, each in the injected `Env`), each naming a different
  adapter, then removes them one at a time from the top; after each removal the binding's adapter is
  the next source's and its `BindingSource` is, in turn, `builder`, `environment`, `stack_file`,
  `working_directory_file`, `user_config_file` (`ess/domains/sdk.yaml:49`).
- With only `INFERENCE_LLM_ADAPTER=mock` in the injected `Env` (feature `testing`), the `llm`
  binding's `BindingSource` is `environment`.
- `Stack::new().adapter(Role::Llm)` is `None`, and `Stack::new().resolve_with(&<empty Env>)` is `Ok`,
  with every role unbound.
- A role bound with `bind` keeps it: `Stack::new().bind(Binding::named(Role::Llm, A, keys))
  .resolve_with(&env)`, with `INFERENCE_LLM_ADAPTER=B` in `env`, binds `llm` to `A` with
  `BindingSource` `builder`, while a role `bind` left alone (`stt`, with `INFERENCE_STT_ADAPTER` in
  `env`) is filled with `BindingSource` `environment`.
- Role keys reach the constructor: a test registers an adapter whose constructor records the keys it
  receives. With `INFERENCE_LLM_ADAPTER=<that name>`, `INFERENCE_LLM_URL=http://${HOST}:8080` and
  `HOST=localhost` in the injected `Env`, the constructor receives `url = "http://localhost:8080"`;
  with the same binding from a stack file whose `[llm]` table has `url = "${HOST}/v1"`, it receives
  `url = "localhost/v1"`.
- `Stack::bind(Binding::named(Role::Llm, <that name>, keys))` binds `llm` with
  `BindingSource::Builder` and the constructor receives `keys`; with the feature `testing`,
  `Stack::bind(Binding::llm(<a MockGenerator over a script>))` binds `llm` and runs the script
  given (the first reply is the script's first line), not the testkit default.
- A typed value without `testing`, in a test that runs under
  `cargo test -p inference --no-default-features --test stack`: the test hand-builds a
  `Synthesiser` of its own, a struct in the test file holding an `Adapter` declared with an id the
  test chooses. Its `adapter()` returns that `Adapter`; its `start()` returns a handle holding
  `SynthesisOutput::start(&adapter)` (`crates/inference-tts/src/lib.rs:704`) that implements
  `SynthesisHandle` (`:1249`): `output` returns it, and `offer_segment`, `end_utterance`,
  `acknowledge_playback` and `discard` forward to its `offer` (`:732`), `end_utterance` (`:754`),
  `acknowledge_playback` (`:900`) and `discard` (`:936`); as an `Iterator` it yields no
  `SynthesisEvent`. The test binds it with
  `Stack::bind(Binding::tts(it))`; after `resolve_with(&Env::new())`, `Stack::adapter(Role::Tts)`
  returns that adapter id with `BindingSource` `builder`.
- Model names, in the injected `Env`: `INFERENCE_LLM_MODEL=qwen3-8b` with a registered `llm`
  adapter gives `Stack::model(Role::Llm) == Some("qwen3-8b")`, and so does `model = "qwen3-8b"` in
  the `[llm]` table; `Stack::bind(Binding::tts(it).model("voice-1"))` gives `Some("voice-1")` for
  `tts`; the value binding of the test above, with no `.model(..)`, gives `None`. With the feature
  `aws`, a stack file `[speech] adapter = "aws"` and no `model` key gives
  `Some("amazon.nova-2-sonic-v1:0")`, and with `INFERENCE_NOVA_SONIC_MODEL=amazon.nova-sonic-v1:0`
  in the `Env` gives that string; with the feature `openai`, `[speech] adapter = "openai"` gives
  `Some("gpt-realtime-2.1")`.
- Speech defaults per provider, each gated by its feature and passing in
  `cargo test -p inference --all-features --test stack`. These are the values the ported
  `examples/realtime-voice/tests/wiring.rs` asserts (`:442-468`, `:504-519`):
  - `openai`: a stack file `[speech] adapter = "openai"` gives a `session_config()` with voice
    `marin`, PCM16 at 24 kHz in and out, `ServerVad` and `auto_interrupt` off, which
    `inference_openai::duplex::capabilities().admit(..)` accepts;
  - `openai-live`: voice `marin`, 24 kHz in and out, `SemanticVad`, `auto_interrupt` on, which
    `inference_openai::live_duplex::capabilities().admit(..)` accepts;
  - `aws`: voice `matthew`, 16 kHz in and 24 kHz out, `ServerVad`, `auto_interrupt` on, which
    `inference_aws::duplex::capabilities(inference_aws::nova_sonic::DEFAULT_MODEL_ID).admit(..)`
    accepts;
  - all three: `auto_response` on, idle limit 30 s, no instructions, no tools, and each equals
    the provider crate's export (`inference_openai::duplex::default_session_config()`,
    `inference_openai::live_duplex::default_session_config()`,
    `inference_aws::duplex::default_session_config()`).
- The `voice` key overrides: with `voice = "cedar"` in the `[speech]` table, and in a second case
  `INFERENCE_SPEECH_VOICE=cedar` in the `Env`, `session_config().voice` is `cedar` and every other
  field equals the adapter's default. With the feature `testing`, a stack binding `speech` by
  `Binding::speech(MockSpeechModel::native(vec![]))` and no
  `voice` key returns `default_session_config()`.
- A stack naming `whispercpp` with the feature off fails with `Error::AdapterNotCompiled`, whose
  message contains `features = ["whispercpp"]`: a test gated `#[cfg(not(feature = "whispercpp"))]`,
  passing in `cargo test -p inference --no-default-features --test stack`, which `task check` runs.
- `Registry::register` of an application adapter under a name the test chooses: with
  `INFERENCE_LLM_ADAPTER=<that name>` in the injected `Env` the stack resolves to it, and
  `Stack::adapter(Role::Llm)` returns that adapter. In the same `#[test]` (R217), a second
  `Registry::register` under that name, and one under the built-in `mock`, each return an
  `Error` with `kind() == inference::ErrorKind::Layer` whose `to_string()` contains the name, and
  the stack still resolves `llm` to the first registration. The `mock` refusal holds with or
  without the feature `testing`, because built-in names are reserved whatever the features
  (R222), so the case is not gated and runs under both `cargo test` commands below.
- A constructor's binding must be its own (R220), in `tests/stack.rs`, each case registering its
  own name:
  - `stack-wrong-role`, whose `llm` constructor returns `Ok(Binding::tts(..).into())` over the hand-built `Synthesiser` of the typed-value case above:
    resolving with `INFERENCE_LLM_ADAPTER=stack-wrong-role` in the injected `Env` fails with
    `kind() == inference::ErrorKind::Layer` whose `to_string()` contains `stack-wrong-role` and
    `llm`.
  - `stack-renamed`, whose `llm` constructor returns `Binding::named(Role::Llm,
    "stack-wrong-role", ..)`: resolving with `INFERENCE_LLM_ADAPTER=stack-renamed` fails with
    `kind() == inference::ErrorKind::Layer` whose `to_string()` contains `stack-renamed` (R219).
- With the feature `testing` on, a stack file with `adapter = "mock"` for each of `stt`, `llm`,
  `tts` and `speech` resolves, each role to the testkit mock (`registry/mock.rs`).
- `cargo tree -p inference-testkit -e normal --all-features` lists no `inference v` line (the
  testkit takes `inference` only as a dev-dependency).
- `cargo test -p inference --test stack` passes, as does
  `cargo test -p inference --features testing --test stack`.
- No resolve reads a credential: with no provider key variable in the injected `Env`, a hosted
  stack resolves without error (the key is read when the adapter opens).
- The AWS client is built when a session opens (R110), gated by the feature `aws`: an `Env` with no
  `AWS_*` variable, a config home in an empty `tempfile` directory (so no profile) and a stack file
  `[speech] adapter = "aws"` resolves with `Ok`; `Stack::adapter(Role::Speech)` is `Some` with
  `BindingSource` `stack_file`, and `session_config()` equals
  `inference_aws::duplex::default_session_config()`. `rg -n 'from_env' crates/inference/src/registry/aws.rs`
  exits 1, and `rg -n 'fn deferred' adapters/inference-aws/src/duplex.rs` matches.
- Typed bindings (R104): `Stack::new().bind(Binding::llm(<mock>).model("m"))` and
  `.bind(Binding::named(Role::Llm, "mock", keys))` both compile and resolve (feature `testing`);
  `let _: Bound<role::Tts> = Binding::tts(<mock>);` and `let _: Binding = Binding::named(..);` are
  typed `let`s in the test, so a changed return type fails to compile.
- `Stack::from_env()` is `Stack::new().resolve_with(&Env::process())`:
  `rg -n 'Stack::new\(\)\.resolve_with\(&Env::process\(\)\)' crates/inference/src/stack.rs` matches.
- `Stack::adapter(Role::Llm)` on a resolved stack returns the bound `inference_core::model::Adapter`
  (`crates/inference-core/src/model.rs:330`) and its `CapabilityRecord` (`model.rs:508`), equal to
  the ones the registry declared.
- `Role` (R194): a test matches each of `Role::Stt`, `Role::Llm`, `Role::Tts`, `Role::Speech`
  with no wildcard arm, so a fifth variant fails to compile.
- What `declare_into` takes (R197), feature `testing`: a stack file with `adapter = "mock"` for
  `stt`, `llm` and `tts` resolves; `adapter_ids()` is three pairs, `Stt`, `Llm`, `Tts`, each id
  equal to `adapter(role)`'s; `Catalog::profile(&stack, stack.profile_id())` is `Some`, and
  `Catalog::adapter(&stack, id)` is `Some` for each of the three ids and `None` for an id no role
  binds.
- The default profile (R201): `Stack::new().resolve_with(&Env::new())` gives a stack whose
  `Catalog::profile(&stack, stack.profile_id())` has `kind()` `ProfileKind::OfflineStrict` and
  `admitted()` `[SameProcess, LocalWorker]` (`policy.rs:267`, `:278`), and `profile_source()` is
  `None`.
- Each `profile` source (R204), one case each, every one giving `ProfileKind::HybridOptIn`:
  - builder: `Stack::new().profile(ProfileKind::HybridOptIn).resolve_with(&env)`, with
    `INFERENCE_PROFILE=offline_strict` in `env`, has `profile_source()` `Some(Builder)`;
  - environment: `INFERENCE_PROFILE=hybrid_opt_in` in the `Env` and a working-directory file with
    `profile = "offline_strict"` gives `Some(Environment)`;
  - stack file: `profile = "hybrid_opt_in"` in the file `INFERENCE_STACK` names gives
    `Some(StackFile)`;
  - working-directory file: the same line in `./inference.toml` gives
    `Some(WorkingDirectoryFile)`;
  - user config file: the same line in `$XDG_CONFIG_HOME/inference/inference.toml` gives
    `Some(UserConfigFile)`.

Its diff lists none of `crates/inference/src/lib.rs`, `crates/inference/src/prelude.rs` and
`crates/inference/Cargo.toml` (story:sdk-crate wrote them once, decision D9 of `api-design:sdk`).

## Out of scope

Fallback between adapters (`docs/design/sdk.md` §4.3); secret storage; `Stack::scripted()`
(story:sdk-testing); the `prices` key (story:sdk-usage-cost).
