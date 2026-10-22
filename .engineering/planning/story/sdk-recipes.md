---
format: aep.planning-md/3
id: story:sdk-recipes
kind: story
status: draft
title: Five named recipes declare, validate and plan a pipeline over a stack
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-payload
- depends_on: story:sdk-stack
- depends_on: story:audio-device-adapter
scope:
- confidence: inferred
  path: adapters/inference-audio-device/src/acknowledging.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/agent.rs
- confidence: inferred
  path: crates/inference-testkit/tests/conform.rs
- confidence: inferred
  path: crates/inference-testkit/tests/target/mod.rs
- confidence: inferred
  path: crates/inference-testkit/tests/target/sdk.rs
- confidence: inferred
  path: crates/inference/src/recipe
- confidence: inferred
  path: crates/inference/src/recipe.rs
- confidence: inferred
  path: crates/inference/tests/fixtures/reference_cascade.rs
- confidence: inferred
  path: crates/inference/tests/recipes.rs
- confidence: inferred
  path: ess/domains/sdk.yaml
revision: 22
---
## Context

`docs/design/sdk.md` §3.3: a recipe is a named topology that declares, validates and plans one
pipeline over a stack. The reference cascade is declared by hand today in
`examples/local-voice-cascade/src/lib.rs:458` (`declare`) and `:515` (`declare_requiring`). Target
T5 (`Recipe::declaration()`). Serves O5.

## Domain relations

`inference.sdk.Instantiation` references one `inference.sdk.Recipe`, one `inference.sdk.Stack` and
one `inference.graph.Pipeline` (`ess/domains/sdk.yaml`). This story declares the command
`inference.sdk.InstantiateRecipe` there, with the outcomes `instantiated`, `role-unbound` (a
recipe whose roles the stack does not all bind) and `profile-denies` (a bound adapter whose
locality the stack's profile does not admit, R213; the UNMAPPED comment of `ess/domains/sdk.yaml`
names it). It declares `inference.sdk.RegisterRecipe` the same way, with the outcomes `registered`
and `name-taken` (a name a built-in or an earlier registration holds), for `Recipe::register`
(R216).

## Planned scope

- `ess/domains/sdk.yaml`: `InstantiateRecipe` and `RegisterRecipe` and their outcomes, both
  removed from the UNMAPPED comment.
- `crates/inference/src/recipe.rs` and `src/recipe/`: `Recipe`, the five recipes `chat`,
  `transcribe`, `speak`, `voice_cascade`, `native_voice`, `with_node`, `expose`, `declaration()`,
  `Recipe::voice_cascade().vad(VadConfig)` (`docs/design/sdk.md` §3.3;
  `crates/inference-pipeline/src/nodes/vad.rs:46`), its default the values of `local_voice_cascade::VAD`
  (`examples/local-voice-cascade/src/lib.rs:133`), and an application recipe (R209, decision D31
  of `api-design:sdk`): `Recipe::custom(name: &str, mode: InteractionMode, declare: impl
  Fn(&mut inference::graph::Builder<'_, '_>) -> Result<(), Error> + Send + Sync + 'static) ->
  Recipe`, `Recipe::role(self, node_name: &str, role: Role) -> Recipe`, its registration
  `Recipe::register(self) -> Result<(), Error>` and its lookup `Recipe::named(name: &str) ->
  Option<Recipe>`. `register` adds the recipe to a process-wide table under its name and refuses
  a name already held, a built-in's or an earlier registration's, with `Error::Layer` naming it;
  `named` returns the built-in or registered recipe of that name. `Recipe` is `Clone`, its declare
  function held in an `Arc`. The table is process-wide, so `named` needs no handle, and has no
  removal (R214): registration is append-only and keyed by name. Each test registers under a
  name of its own, and a test that asserts the taken-name refusal does so inside the same
  `#[test]` as the first registration of that name, so tests running in parallel in one process
  cannot interfere.
  Tests bind the testkit mocks by the adapter name `mock` that story:sdk-stack registers
  (`crates/inference/src/registry/mock.rs`); this story adds no registration.
- Declaring into a caller's declaration (R191). `Recipe::declare_into(&self, declaration: &mut
  Declaration, catalog: &dyn Catalog, profile_id: ProfileId, adapters: &[(Role, AdapterId)],
  requires: &[(Role, &[Capability])]) -> Result<PipelineId, Error>` declares the recipe's nodes,
  ports, edges and exposures into the caller-owned `declaration` through
  `Declaration::build(catalog, profile_id, ..)` (`crates/inference-pipeline/src/builder.rs:554`),
  binds each role's node to that role's adapter through `PipelineBuilder::bind` (`:407`), and
  gives each role named in `requires` that node's requirements in place of the recipe's default
  (for `voice_cascade`, the reference cascade's: none for `stt`, output streaming for `agent`,
  incremental input and output streaming for `tts`, `examples/local-voice-cascade/src/lib.rs:559-565`).
  The caller builds `catalog` and the profile, so the profile's admitted localities and the
  catalog reach planning: the caller validates the returned `PipelineId` and plans it with
  `declaration.plan(validated, catalog)` (`crates/inference-pipeline/src/planning.rs:206-210`),
  as `declare_requiring` does today (`lib.rs:674-680`). The recipe's own nodes and ports are
  those of the returned pipeline: a caller reads them through
  `declaration.nodes()` and `ports()` (`crates/inference-pipeline/src/declaration.rs:708`,
  `:713`) filtered by `pipeline_id()` (`crates/inference-pipeline/src/lib.rs:844`, `:896`),
  because a shared declaration holds every pipeline declared in it.
  - The argument types are reached through the module re-exports story:sdk-crate owns:
    `inference::pipeline::{Catalog, Declaration, PipelineId}`,
    `inference::core::policy::ProfileId` and `inference::core::model::{AdapterId, Capability}`;
    `Role` is story:sdk-stack's `inference::Role` (R195).
  - Refusal (R195, R196). Before declaring anything, `declare_into` looks `profile_id` up with
    `Catalog::profile` and every id in `adapters` with `Catalog::adapter`
    (`crates/inference-pipeline/src/declaration.rs:29`, `:32`), the lookups `Declaration::build`
    (`builder.rs:560`) and `PipelineBuilder::bind` (`:411`) make later. A miss returns
    `Error::Layer` in the terms of `docs/design/sdk.md` §9: its message names the missing
    `ProfileId`, or the missing `AdapterId` and its role, and its `source()` is the pipeline's
    `ProfileNotFound` (`crates/inference-core/src/policy.rs:188`) or `AdapterNotFound`
    (`crates/inference-pipeline/src/lib.rs:749`). The caller's declaration then holds the same
    pipelines, nodes, ports and bindings as before the call. The check comes first because `build`
    inserts the pipeline before it runs the closure (`builder.rs:564-571`), so an
    `AdapterNotFound` from `bind` inside the closure would leave the recipe's pipeline and nodes
    behind.
  - Roles (R202). `declare_into` binds only the roles the recipe has nodes for (`chat`: `llm`;
    `transcribe`: `stt`; `speak`: `tts`; `voice_cascade`: `stt`, `llm`, `tts`; `native_voice`:
    `speech`) and ignores every other pair in `adapters`. A role the recipe needs with no pair is
    refused in the same up-front check: `Error::Layer` whose message names the role, with `source`
    `None`, and the declaration unchanged.
  - A recipe's roles (R207) are the roles of its role-bound nodes. The five recipes' role-bound
    nodes are the model nodes listed above. `with_node(name, node)` keeps the role of the node it
    replaces; a node it adds under a new name has no role. `Recipe::role(self, node_name: &str,
    role: Role) -> Recipe` makes a node role-bound, and an application recipe
    (`Recipe::custom(name, mode, declare)`, `declare` a closure over the `inference::graph`
    builder, registered with `register` and found with `named` as above) names its role-bound
    nodes that way. A node without a role needs
    no binding: `declare_into` binds nothing to it, and neither the R202 check nor the
    instantiation's `RoleUnbound` check asks for one.
  - Where the arguments come from (R197). The instantiation calls `recipe.declare_into(&mut
    declaration, &stack, stack.profile_id(), &stack.adapter_ids(), &[])`, over the
    `impl Catalog for Stack`, `Stack::profile_id` and `Stack::adapter_ids` of story:sdk-stack.
    Before that call it checks the recipe's roles against `stack.adapter_ids()` and returns
    `Error::RoleUnbound` naming the first unbound role in the recipe's role order (the
    `role-unbound` outcome): for a built-in, the order the Roles list above gives; for a custom
    recipe, the order of its `Recipe::role` calls (R215). `declare_into`'s `Layer` refusal of a
    missing role names the first in the same order. The instantiation returns `RoleUnbound` so an
    application sees the fix §9 names; `declare_into`'s `Layer` refusal is for a direct caller.
    When planning refuses with `PlanPipelineError::LocalityDenied`
    (`crates/inference-pipeline/src/planning.rs:126`, `:276`), the instantiation returns
    `Error::ProfileDenies` (story:sdk-crate, R203) built from it: the role of the denied node
    (`LocalityDenied::node_id`, `crates/inference-pipeline/src/lib.rs:477`) as its lowercase
    name, `stt`, `llm`, `tts` or `speech` (R212), the adapter's registered name, its `locality`,
    the stack's `ProfileKind`, and the `LocalityDenied` as `source` (the `profile-denies`
    outcome, R213).
  - `Recipe::declaration(&self) -> Declaration` is the stack-free form, for inspection and the
    tests that read a recipe's topology. On a fresh `Declaration` it runs the crate-private
    declare step `declare_into` runs after its checks, over a crate-private catalog holding one
    profile, declared by `DeploymentProfile::declare`
    (`crates/inference-core/src/policy.rs:220`) with `ProfileKind::OfflineStrict` and
    `Egress::Denied`, and no adapter, with no pair and no requirement, so it binds nothing and
    the R202 role check does not apply. Its one pipeline holds the recipe's nodes, ports, edges,
    exposures and feedback loops, and no binding; it is not planned.
- Tools in `native_voice`. `Recipe::tools(self, impl IntoIterator<Item = (Tool, Box<dyn
  ToolExecutor>)>)` gives a recipe the pairs story:sdk-agent's builder `tool` collects. The
  `native_voice` topology is `DuplexNode` plus the runtime's existing tool nodes, with no source or
  sink node (R159): its ingresses are the node's audio in and control in, its one egress the
  node's `events`, and the host or the application feeds and plays them (story:sdk-agent). A
  sink node there would be fed by nothing and fail check 6
  (`crates/inference-pipeline/src/validation.rs:276-327`). The tool nodes are
  (`crates/inference-pipeline/src/nodes/tool_authority.rs`, `tool_executor.rs`, exercised by
  `crates/inference-runtime/tests/tool_nodes.rs`), wired as
  `crates/inference-runtime/tests/tool_feedback_loop.rs:225-287` wires them around a model:
  - The feedback bound: the `DuplexNode`'s `tool_call` output (story:sdk-duplex-node) goes to the
    authority's `proposed` input through `connect_feedback(&tool_call, &proposed, 4,
    OverloadPolicy::Fail, MAX_TOOL_ITERATIONS)` (`:270-279`), because the loop through the executor
    returns to the node. `MAX_TOOL_ITERATIONS` is a `NonZeroUsize` of 4, a `pub const` in
    `recipe.rs`.
  - The authority:
    - It is built as the test helper builds it (`tool_feedback_loop.rs:372-383`):
      `ToolAuthority::new(run_id, <the recipe's tools>)` (`tool_authority.rs:314`), then
      `.bounded_by(&declaration, &planned, <the authority's node id>)` (`:355`).
    - `bounded_by` takes the iteration rule from that feedback loop. It cannot fail here, because
      the recipe declares the loop; an `IterationRuleNotDeclared` would be a recipe bug, returned
      as `Error::Layer`.
    - Then `.rejecting_on(<the authority's rejected port id>)` (`:393`).
    - Its `authorized` output goes to the executor's `authorized` input with capacity 4 and
      `OverloadPolicy::Fail`.
  - The executor:
    - `ToolExecutorNode::new(run_id, Arc<Dispatch>, <the tools' ids>, 30 s)`
      (`tool_executor.rs:224`), then `.answering_to(&authority.handle())` (`:257`, `:400`).
    - `Dispatch` is a private `ToolExecutor` (`crates/inference-tools/src/executor.rs:24`) in
      `src/recipe/`. It routes each call by its tool id to the executor the recipe was given for
      that tool.
  - The rejection port: the executor's `outcomes` and the authority's `rejected` outputs both go to
    the `DuplexNode`'s `tool_result` input, each with capacity 4 and `OverloadPolicy::Fail`
    (`:285-286`). So a call the authority refuses (a denied confirmation, or an iteration
    past the bound) is answered too. A tool the recipe was not given never leaves the node: the
    `DuplexNode`, built with the recipe's tools, answers it as denied (story:sdk-duplex-node).
  - The confirmation handle (R101). A tool with `requires_confirmation` is held Proposed until the
    handle answers (`tool_authority.rs:16-19`, `:48-49`). The instantiated recipe returns the
    authority's `AuthorityHandle` (`:400`, `Clone`) to story:sdk-agent, which answers held calls
    with `confirm` and `deny`, and denies every held call before it closes the session.
  - The call-id map handle (R102): the instantiated recipe also returns the `DuplexNode`'s
    `CallIds` handle (story:sdk-duplex-node), from which story:sdk-agent reads the `SpeechCallId`
    of each `ToolCallId`.
  A recipe given no tools wires the same nodes with an empty tool set, so every proposed call is
  rejected and answered.
- One egress for the agent (R142). `native_voice` exposes exactly one egress, the `DuplexNode`'s
  `events` output (story:sdk-duplex-node). Every payload story:sdk-agent reads leaves there,
  including the proposal the node also sends on `tool_call` and the closed notification. So the
  session delivers them in the node's emission order: one port, one exposure queue in arrival
  order (`crates/inference-runtime/src/queue.rs:8-12`). `tool_call` is an edge only and is not
  exposed. A second egress would be drained in index order, not emission order
  (`crates/inference-facade/src/live.rs:976-993`).
- The cascade's audio (R167, R168, R181). `voice_cascade` keeps the reference cascade's exposures,
  microphone in, `committed` and `to_app` out (`examples/local-voice-cascade/src/lib.rs:622-624`),
  and has no audio egress: an egress on the speech node's audio is not possible as the node
  stands, since `SpeechNode` takes exactly one output edge at its audio port, which the speaker
  edge uses (`crates/inference-testkit/src/cascade/speech.rs:701`). Its speaker `Sink` node is
  the reference cascade's `AcknowledgingSink` with its turns port (`:875`), playing through the
  Agent's sink slot, the route native audio takes (story:sdk-agent), instead of a `Speaker` opened
  before the run (`adapters/inference-audio-device/src/acknowledging.rs:652`). The turn handling,
  receipts and records are the existing code; no second copy is written:
  - story:audio-device-adapter (active) lands on `adapters/inference-audio-device`, which holds
    `src/acknowledging.rs`, so this story depends on it (R188) and builds on its finished file.
  - `adapters/inference-audio-device/src/acknowledging.rs`: a `pub trait Playout: Send` with the
    five `Speaker` methods the exchange calls, same signatures (`try_push`, `finish`, `stop`,
    `played`, `queued`; `playout.rs:476-529`), and `impl Playout for Speaker`, which delegates.
    `Playing` holds a `Box<dyn Playout>` where it holds a `Speaker` today. `AcknowledgingSink::new`
    and `with_turns` keep their signatures and behaviour; `AcknowledgingSink::over(run_id,
    Box<dyn Playout>)` (new) builds the same sink over any playout. Two builder methods, default
    unchanged: `cutting(self, bool)`, where `false` makes the sink take each `TurnOpened` and act
    on none, so it stops nothing, makes no generation stale and records no interruption; and
    `on_interruption(self, impl Fn(&Interruption) + Send + 'static)`, called each time the sink
    records an interruption (`:446`, `:474`). The handle stays `AcknowledgingHandle`, with the
    same methods and results.
  - `crates/inference/src/recipe/speaker.rs` (new): `pub(crate) trait AudioRoute: Send + Sync {
    fn route(&self, frame: AudioFrame); fn played(&self) -> u64; fn cut(&self); fn
    interrupted(&self); }` (`played`: the frames routed so far that have been handed out; `cut`:
    stop the audio routed and not yet handed out; `interrupted`: tell the application of a
    barge-in), and `pub(crate) struct RoutedPlayout`, the `Playout` over an `Arc<dyn AudioRoute>`:
    `try_push` routes the frame and never answers `Full`; `played` is the end, on the frames'
    clock, of the last routed frame `played()` counts as handed out; `queued` is the samples of
    the frames routed and not yet handed out; `stop` calls `cut`, forgets those frames and answers
    the played position, as `Speaker::stop` does (`playout.rs:512-517`); `finish` does nothing.
    The sink is `AcknowledgingSink::over(run_id, Box::new(RoutedPlayout::new(route)))
    .with_turns(<the sink's turns port>).cutting(auto_interrupt).on_interruption(<a closure
    calling route.interrupted()>)`. So the planned pipeline keeps the fixture's nodes, ports,
    edges and traits, and the sink keeps the reference sink's behaviour: a `TurnOpened` cuts the
    queued and in-flight frames of every generation begun before it, and the handle counts
    interruptions, stale frames and pushed frames. `AudioRoute` and `RoutedPlayout` stay
    crate-private; the file's `#[cfg(test)]` module holds the sink and barge-in tests below
    (R187).
  - The LLM node's own cut (R181, off). At a `TurnOpened` while its reply is unplayed, the
    `AgentNode` supersedes its generation (`crates/inference-testkit/src/cascade/agent.rs:534-543`,
    in `crates/inference-pipeline/src/nodes/agent.rs` after story:sdk-model-nodes), and the
    supersede stops the sink's audio (`acknowledging.rs:457-476`). So `AgentNode` gains
    `keeping_replies(self)`: at a `TurnOpened` it completes the current generation, as it does
    today when every segment was played (`agent.rs:537-540`), and never supersedes it. With
    `auto_interrupt` off, `voice_cascade` builds its `AgentNode` with it.
  - The instantiation of `voice_cascade` takes the `Arc<dyn AudioRoute>` and the `auto_interrupt`
    the Agent hands it at open and at each switch, through a `pub(crate)` entry in
    `src/recipe/speaker.rs`, so no public signature names the crate-private route (R187), and
    returns the sink's `AcknowledgingHandle`
    beside its other handles (`instantiation.sink()`); every other recipe ignores both. With no
    sink attached a cascade reply reaches the application as `Event::Audio`, and with a device run
    the speaker (R167).
  - The sink sends receipts through `PlaybackPayload` and the `Receipt` variant, which exist
    behind `audio-host` (story:sdk-payload). So `src/recipe/speaker.rs` and `voice_cascade`'s
    instantiation sit behind `audio-host` (`testing` implies it, story:sdk-crate); without it,
    instantiating `voice_cascade` returns `Error::AdapterNotCompiled` naming `audio-host`, and
    `Recipe::voice_cascade().declaration()` and `declare_into` still declare.
- Exposure capacities (R111). Every exposure the five recipes declare, ingress or egress, has
  capacity `EXPOSURE_CAPACITY`, a `pub const NonZeroUsize` of 64 in `recipe.rs`, under
  `OverloadPolicy::Pause`, the values the reference cascade uses
  (`examples/local-voice-cascade/src/lib.rs:551-552`). `Recipe::exposure_capacity(self,
  NonZeroUsize)` sets it for every exposure of the recipe. Both are public (R123): the capacity
  trades latency against buffering, and a smaller value bounds how far the application falls
  behind before the model's output pauses (`docs/design/sdk.md` §3.3, D27 of `api-design:sdk`).
  story:sdk-agent tests a full egress
  with `NonZeroUsize::MIN`.
- `crates/inference/src/recipe.rs` gains, from story:sdk-usage-cost, the `Serving` wrapping of a
  role's node behind `cost`; that story comes after this one.
- `crates/inference/tests/fixtures/reference_cascade.rs` (new): the declaration code of
  `local_voice_cascade::declare` as it stands after story:sdk-payload, copied before story:sdk-examples
  ports the example, so the comparison below survives the port.
- `crates/inference/tests/recipes.rs` (stub from story:sdk-crate, filled here).
- Conformance: `crates/inference-testkit/tests/conform.rs`, `tests/target/mod.rs` (existing) and
  `tests/target/sdk.rs` (after story:sdk-stack, which created the last).

## Acceptance

Conformance scenarios synthesized by `ess verify conform synthesize` from `InstantiateRecipe` and
`RegisterRecipe` and answered by the testkit target, each passing in
`cargo test -p inference-testkit --features run --test conform`:

- `inference.sdk.InstantiateRecipe/outcome/instantiated`
- `inference.sdk.InstantiateRecipe/outcome/role-unbound`
- `inference.sdk.InstantiateRecipe/outcome/profile-denies` (R213)
- `inference.sdk.RegisterRecipe/outcome/registered` (R216)
- `inference.sdk.RegisterRecipe/outcome/name-taken` (R216)

The sixteen arrays present at the story's start (456 ids on main at `dc8e13e`) all still pass;
`SDK_SCENARIOS` is not counted in that number.

And Rust tests in `crates/inference/tests/recipes.rs`, over the testkit mocks bound by the name
`mock`, passing in `cargo test -p inference --features testing --test recipes`:

- `Recipe::voice_cascade().vad(VadConfig { threshold: 0.9, .. })`: the instantiated VAD node is
  built with that config, shown by its behaviour: a synthetic utterance at 0.5 of full scale yields
  no `SpeechStarted` under it and one under the default `VAD` (threshold 0.05).

- `Recipe::voice_cascade()` plans a pipeline with the same nodes, ports, edges and traits as the
  fixture `reference_cascade.rs`.
- `Recipe::native_voice()` over the duplex mock plans a pipeline of one `DuplexNode`, one
  `ToolAuthority` and one `ToolExecutorNode`, with no source or sink node and the `tool_call` to
  `proposed` edge a feedback edge; `declaration.validate` reports no finding, check 6 and the
  cycle included (R159). Its declaration exposes exactly one
  egress, on the `DuplexNode`'s `events` port (R142).
- A tool call over `native_voice` is answered by the provider's call id (R102). The mock is
  `MockSpeechModel::native(vec![<a Reply::Call turn naming the tool "lookup">])`
  (the mock names its first call `call_1`, `crates/inference-testkit/src/duplex/mock.rs:491-492`), and the recipe is `Recipe::native_voice().tools([<a hand-built "lookup" Tool
  and an executor answering "Berlin">])`, opened as a live session:
  - The model's `submit_tool_result` receives `SpeechCallId("call_1")` and the executor's answer.
  - The session's events hold `ToolCallProposed` before the reply's response transcript, and the
    instantiation's `CallIds` handle maps that event's `ToolCallId` to `SpeechCallId("call_1")`.
    story:sdk-agent shows them as `ToolCalled { speech_call_id: Some("call_1"), .. }` then `Said`.
- Answered by a rejection (R101): with no `.tools(..)`, the same call is answered under
  `SpeechCallId("call_1")` by an error result whose text names the rejection, the executor's
  `Dispatch` is never called, and the response still streams.
- The feedback bound: `Recipe::native_voice().declaration().feedback_loops()` holds one loop, with
  `max_iterations` 4, whose edge runs from the `DuplexNode`'s `tool_call` port to the authority's
  `proposed` port. The instantiated authority prepares, which shows that `bounded_by` found the
  loop. The duplex mock proposes one call per response (`mock.rs:389-395`), so no run can propose
  a fifth. `rg -n 'MAX_TOOL_ITERATIONS' crates/inference/src/recipe.rs` matches.
- A confirmation is held and answered. A `lookup` tool with `requires_confirmation` is held:
  - `instantiation.authority().held()` holds that call, and the executor has not run.
  - `deny(call_id)` answers the model under `call_1` with a rejection.
  - In a second case, `confirm(call_id)` runs the executor.
  - In a third case, closing the session after `deny`ing every held call returns `Ok(())` within
    5 s.
- The closed notification reaches the session: over the same mock built with `.closes()`, the
  session delivers `SessionEvent::SpeechSessionClosed` once, after the reply.
- `chat()`, `transcribe()` and `speak()` each plan the topology of `docs/design/sdk.md` §3.3's
  table: their node roles and the modality of each exposed end are asserted.
- `recipe.declaration()` is the stack-free form (R197): for each of the five recipes it holds one
  pipeline and `bindings().count()` is 0, and its node names, port names and edge endpoints equal
  those of the pipeline `declare_into` declares over a stack resolved with `adapter = "mock"` for
  every role (`registry/mock.rs`, story:sdk-stack), which validates and plans.
- `declare_into` (R191): into a `Declaration` that already holds another pipeline,
  `Recipe::chat()` declared into it first, `Recipe::voice_cascade().declare_into(&mut
  declaration, &catalog, profile_id, &[(Role::Stt, r), (Role::Llm, g), (Role::Tts, s)], &[])`
  returns a second `PipelineId`; the nodes whose `pipeline_id()` is that id are the eleven of the
  copied fixture, by name, and validating and planning that id with `&catalog` succeeds. The same
  call with `&[(Role::Stt, &[Capability::IncrementalInput, Capability::OutputStreaming])]` and a
  recogniser adapter claiming neither validates and fails planning with
  `PlanPipelineError::Mismatch` naming the node `stt`.
- A refused `declare_into` leaves the declaration as it was (R196). Both cases start from a
  `Declaration` holding `Recipe::chat()`'s pipeline and record `pipelines().count()`,
  `nodes().count()`, `ports().count()` and `bindings().count()` (`declaration.rs:703`, `:708`,
  `:713`, `:780`) before the call:
  - Missing profile: `Recipe::voice_cascade().declare_into(..)` with a `profile_id` of a profile
    declared by `DeploymentProfile::declare` but not held by `catalog`, and the three adapters
    held, returns an `Error` with `kind() == inference::ErrorKind::Layer` (story:sdk-crate), whose
    `to_string()` contains that `ProfileId`, and whose `source()` downcasts to `ProfileNotFound`
    with that id.
  - Missing adapter: the same call with the catalog's profile and a `tts` adapter id the catalog
    does not hold returns an `Error` with `kind() == inference::ErrorKind::Layer`, whose `to_string()`
    contains that `AdapterId` and `tts`, and whose `source()` downcasts to `AdapterNotFound` with
    that id.
  - Missing role (R202): the same call with the catalog's profile and only the `Stt` and `Llm`
    pairs returns an `Error` with `kind() == inference::ErrorKind::Layer` whose `to_string()` contains `tts`.
  - In each, the four counts equal those recorded before the call.
- An extra role is ignored (R202): `Recipe::voice_cascade().declare_into(..)` with the three
  cascade pairs and a fourth `(Role::Speech, <an adapter the catalog holds>)` returns `Ok`; the
  new pipeline's bindings (`bindings()` filtered by its `pipeline_id`) are three, none carrying
  the speech adapter's id, and it validates and plans.
- An unbound role on the instantiation path (R205), a unit test in `src/recipe/speaker.rs`'s
  `#[cfg(test)]` module under `cargo test -p inference --features testing --lib`: instantiating
  `voice_cascade` through the `pub(crate)` entry over a stack resolved with `stt` and `llm` bound to
  the mocks and no `tts` binding returns an `Error` with
  `kind() == inference::ErrorKind::RoleUnbound` whose `to_string()` contains `tts`.
- A custom recipe's roles (R207), a unit test in `src/recipe.rs`'s `#[cfg(test)]` module under the
  same command: `Recipe::custom("ask", InteractionMode::TurnBased, <declares a text ingress, one `AgentNode` named "agent" and
  a text egress>).role("agent", Role::Llm)` instantiated over a stack resolved with only `llm`
  bound to the mock returns `Ok`, and its pipeline holds one binding, on `agent`.
- A custom recipe's roles through the public call (R211), in `tests/recipes.rs`, over
  `Recipe::custom("ask", InteractionMode::TurnBased, <a declare written as that one>).role("agent", Role::Llm)`:
  - `declare_into(&mut declaration, &catalog, profile_id, &[(Role::Stt, r)], &[])`, with no
    `llm` pair, returns an `Error` with `kind() == inference::ErrorKind::Layer` whose
    `to_string()` contains `llm`, and the declaration's four counts are unchanged.
  - The same call with `&[(Role::Llm, g)]` returns `Ok`, and the new pipeline holds one binding,
    on `agent`.
- An application recipe is registered and found by name (R209): `Recipe::custom("ask-reg", InteractionMode::TurnBased,
  <the same declare>).role("agent", Role::Llm).register()` returns `Ok(())`;
  `Recipe::named("ask-reg")` returns `Some`, whose `declare_into` over the catalog with the `llm`
  pair returns a pipeline that validates and plans; a second `register()` under `ask-reg`, and
  one under `chat`, each return an `Error` with `kind() == inference::ErrorKind::Layer` whose
  `to_string()` contains the name; `Recipe::named("chat")` returns the built-in, and
  `Recipe::named("absent")` returns `None`. All of it runs in one `#[test]`, and `ask-reg` is a
  name no other test registers (R214).
- A custom recipe reports its roles in `role` order (R215), in `tests/recipes.rs`:
  `Recipe::custom("ask-order", InteractionMode::TurnBased, <declares a text ingress, an
  `AgentNode` named "agent", a synthesiser node named "speaker" and an audio egress>)
  `.role("speaker", Role::Tts).role("agent", Role::Llm)`, whose `declare_into` is given only an
  `stt` pair, so neither role is bound, returns an `Error` with
  `kind() == inference::ErrorKind::Layer` whose `to_string()` contains `tts` and not `llm`.
- `with_node("tts", …)` replaces the implementation of that node and no other.
- `expose("assistant_text")` adds that exposed end to the planned pipeline and nothing else.
- Exposure capacities: in the `declaration()` of each of the five recipes, every exposure has
  capacity 64 and `OverloadPolicy::Pause`; after `.exposure_capacity(NonZeroUsize::MIN)` every
  exposure of `native_voice` has capacity 1 and the policy is unchanged.
- The cascade sink (R168, R181), unit tests in `src/recipe/speaker.rs` under `cargo test -p
  inference --features testing --lib`, over a test-local `AudioRoute` that records each frame
  routed, counts its `cut` and `interrupted` calls, drops the frames not yet handed out at a `cut`,
  and whose `played()` the test sets:
  - Two frames of one segment of generation `g`, then its `SynthesisCompleted`: both frames reach
    the route in order, and no receipt is sent while `played()` is 1; once it is 2, one receipt
    naming the segment is sent.
  - The same two frames with `played()` held at 0, then a `TurnOpened` on the turns port, then a
    third frame of `g`: `cut` is called once and `interrupted` once, the third frame never reaches
    the route, no receipt is sent for the segment, and the handle reports `interruptions().len()`
    1 and `stale_frames().len()` 1.
  - The same input to a sink built with `cutting(false)`: `cut` and `interrupted` are never
    called, all three frames reach the route, and `interruptions()` is empty.
- Cascade barge-in (R181, R187), a unit test in the same `#[cfg(test)]` module of
  `src/recipe/speaker.rs` under the same command, since the route is crate-private:
  `voice_cascade` instantiated through the `pub(crate)` entry over the cascade mocks and that
  test-local route, opened as a live session, with `played()` held at 0.
  The test sends the frames of `inference_testkit::stt::synthetic_audio::synthesize(&FIXTURE[..2])`
  (`synthetic_audio.rs:77`, `:85`, one turn), waits until the route has recorded a frame, then
  sends those of `FIXTURE[2..3]`, the second tone, which opens turn 2 while reply 0 is unplayed:
  - With `auto_interrupt` on: `cut` is called, `instantiation.sink().interruptions().len()` is 1,
    and once the test raises `played()` to every frame routed, no frame routed before the cut is
    handed out.
  - With `auto_interrupt` off, the same input: `cut` is never called, `interruptions()` is empty,
    and the frames routed before turn 2 opened are all still routed and handed out once the test
    raises `played()`.
  The frame marks that tell a synthesiser frame apart, and `FakeHost::played_frames()`, come later
  from story:sdk-agent, which repeats this case on the device (R181).
- `cargo test -p inference-audio-device --all-features` and `cargo test -p local-voice-cascade
  --features local` pass with `git diff` empty for their `tests/` (R181: `new` and `with_turns`
  unchanged).

Its diff lists none of `crates/inference/src/lib.rs`, `crates/inference/src/prelude.rs` and
`crates/inference/Cargo.toml` (story:sdk-crate wrote them once, decision D9 of `api-design:sdk`).

## Out of scope

Topologies C and E of `docs/design/inference.md` §14 (speech-aware hybrid, duplex text cascade).
