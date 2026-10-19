---
format: aep.planning-md/3
id: story:pipeline-declaration
kind: story
status: draft
title: 'Declare a pipeline: nodes, typed ports, bounded edges and bindings through the typed builder'
summary: inference-pipeline crate and the inference.graph declaration commands; the testkit answers them
relations:
- decomposes: epic:graph-minimum
- serves: vision:inference
- depends_on: story:workspace-skeleton
- depends_on: story:testkit-conformance-target
- depends_on: story:inbound-text-spans-and-envelope
- depends_on: story:llm-contract
scope:
- confidence: cited
  path: crates/inference-core/src/lib.rs
- confidence: inferred
  path: crates/inference-core/src/policy.rs
- confidence: cited
  path: crates/inference-pipeline
- confidence: cited
  path: crates/inference-testkit/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/src/graph.rs
- confidence: inferred
  path: crates/inference-testkit/src/policy.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/graph.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/policy.rs
- confidence: cited
  path: ess/SKIPPED.md
revision: 21
---
# Story: pipeline-declaration

## Outcome

A Rust consumer declares a pipeline of named nodes with typed ports, connects an output port to an input port with finite buffering, binds a node to an adapter and reads the declaration back; the testkit answers the same declaration as `inference.graph` commands and views, so the conformance scenarios for declaration pass.

## Context

First story of `epic:graph-minimum` (stage P1, `docs/design/inference.md:1159`). It creates `crates/inference-pipeline`, the crate for nodes, typed graph descriptions and static validation whose only dependencies are `inference-core` and declared port types (`docs/design/inference.md:241`), and gives it the declaration half of `ess/domains/graph.yaml`: `inference.graph.Pipeline`, `Node`, `Port`, `Edge`, `Binding`, their identifier newtypes and the enums `NodeRole`, `PortDirection`, `Essentiality`, `Delivery`, `InteractionMode`, `OverloadPolicy`. A pipeline definition is not an execution (`docs/design/inference.md:519`); this story stops at the declaration.

Two surfaces produce the same declaration:

- the commands of `ess/domains/graph.yaml` (`DeclarePipeline`, `AddNode`, `AddPort`, `ConnectPorts`, `BindNode`), answered by the testkit conformance runner for the synthesized scenarios;
- the programmatic typed builder of `docs/design/inference.md:490` (§9.3), which gives a Rust consumer compile-time help: a port handle carries its direction and modality and an edge capacity is a `NonZeroUsize`, so an unbounded edge or a dangling id cannot be written through it.

`inference.graph.Pipeline` references an `inference.policy.DeploymentProfile` (`ess/domains/graph.yaml`, relation `profile`), so declaring a profile is a precondition of declaring a pipeline. The epic Scope therefore owns the `inference.policy` types and the scenario `inference.policy.DeclareProfile/outcome/declared`, and this story is where they land: the ESS artifact places `inference.policy` in `inference-core` (`executable-system-specification:inference`, answer to Q1), so this story adds `ProfileId`, `ProfileKind`, `Egress`, `DeploymentProfile` and the errors `ProfileOutsideTable` and `ProfileNotFound` (`ess/domains/policy.yaml`) to `inference-core` and makes the runner answer `inference.policy.DeclareProfile` and the view `inference.policy.Profiles`. `ess/SKIPPED.md` already routes that scenario to `epic:graph-minimum` (`story:testkit-conformance-target`, step 3).

This story depends on `story:workspace-skeleton` for `inference-core` (`inference.model.Capability`, `Modality`, `AdapterId`) and on `story:testkit-conformance-target` for the `ConformanceTarget` runner, the in-memory stores and `ess/SKIPPED.md`, which this story extends in `crates/inference-testkit/tests/conform.rs` and `crates/inference-testkit/src/`.

Four further stories of `epic:contracts-and-testkit` write the same files as this one. `story:tts-contract` adds its optional `inference-tts` dependency line to `crates/inference-testkit/Cargo.toml`, the one table this story's `inference-pipeline` dependency line also enters; the two are unordered, the collision is named in both bodies, and the two lines are different entries of one table. `story:outbound-text-generations-and-segments` and `story:inbound-text-spans-and-envelope` fill their domain's file under `crates/inference-testkit/tests/target/` and their store module under `crates/inference-testkit/src/` (each adding one `mod` line to `src/lib.rs`, the one line all of them share), remove their ids from `ess/SKIPPED.md` and add `Generation` and the event envelope to `crates/inference-core`, where this story adds the graph and policy commands, removes its own ids and adds the `inference.policy` types. `story:llm-contract` adds the `llm` feature and its mock to `crates/inference-testkit` (`Cargo.toml`, `src/lib.rs`), where this story adds the `inference-pipeline` dependency and the graph stores. The `depends_on` edges to `story:inbound-text-spans-and-envelope` (which depends on the outbound story) and to `story:llm-contract` record those shared files as their reason; this story is the head of the graph chain, so the later graph stories inherit the order.

## Steps

1. Specification: nothing to add; `ess/domains/graph.yaml` and `ess/domains/policy.yaml` declare every noun of this story. Run `ess specify validate --path ess` before and after; it stays `valid`.
2. Contract: the types above in `crates/inference-pipeline` with the fields of `ess/domains/graph.yaml`, `Edge.capacity` held as `NonZeroUsize` (invariant `capacity > 0` on `inference.graph.Edge`); the `inference.policy` types in `inference-core`, in a module of their own.
3. Behaviour: the five declaration commands as functions over a declaration value, each refusing what graph.yaml refuses and creating nothing: `DeclarePipeline` an unknown profile (`unknown-profile`, `inference.policy.ProfileNotFound`); `AddNode` an unknown pipeline (`unknown-pipeline`, `inference.graph.PipelineNotFound`) and a pipeline that is not Declared (`not-declared`, `inference.graph.PipelineNotReady`); `AddPort` an unknown node (`unknown-node`, `inference.graph.NodeNotFound`); `ConnectPorts` an unknown pipeline, an unknown source port (`unknown-source`, `inference.graph.PortNotFound`), a capacity below one (`unbounded`, `inference.graph.UnboundedEdge` with the submitted value), a pipeline that is not Declared and a source port that is not an output port (`source-not-output`, `inference.graph.IncompatibleEdge`); `BindNode` an unknown pipeline, node or adapter (`unknown-adapter`, `inference.graph.AdapterNotFound`) and a pipeline that is not Declared; `inference.policy.DeclareProfile` a row outside the deployment-profile table (`wrong-egress`, `remote-not-opted-in`, `offline-admits-cluster`, `inference.policy.ProfileOutsideTable`). The sink port's existence and direction, the modality of the two ports and pipeline membership are not refused by a command (graph.yaml, the comment on `ConnectPorts`) and stay checks 1 and 2 of validation (`story:pipeline-validation`).
4. Builder: the typed builder emits the same declaration value; a test shows the builder and the commands produce equal declarations for the same input.
5. Testkit: the conformance runner of `inference-testkit` answers the commands and views named under Acceptance, emitting exactly the events graph.yaml and policy.yaml declare and nothing else (every scenario asserts `expect_no_event` for every other event of the system); the graph and policy handling goes in the per-domain files `story:testkit-conformance-target` pre-creates under `crates/inference-testkit/tests/target/` (`graph.rs`, `policy.rs`, each answering `unsupported` until filled), whose `mod.rs` already dispatches by domain, so this story fills those two files and the `graph` and `policy` store modules under `src/` and changes neither `conform.rs` nor the dispatch.
6. `ess/SKIPPED.md` loses exactly the 14 ids named under Acceptance as passing at this story; the 9 under `## Scenarios that pass with story:pipeline-validation` stay, naming `story:pipeline-validation` (`story:testkit-conformance-target`, step 3: every later story removes the ids it makes pass, and the conformance test asserts `unsupported` equals that file).

## Domain relations

All read from `ess/domains/graph.yaml` unless another file is named; every one is `inferable`.

- `Pipeline → Node`, one-to-many, zero allowed (an empty pipeline validates in `inference.graph.Pipeline/transition/validate/by/inference.graph.ValidatePipeline/validated`), pipeline owns node: entity `inference.graph.Pipeline`, relation `nodes`.
- `Pipeline → Edge`, one-to-many, zero allowed, pipeline owns edge: relation `edges`.
- `Pipeline → Binding`, one-to-many, zero allowed, pipeline owns binding: relation `bindings` (decision D3 of `executable-system-specification:inference`).
- `Pipeline → DeploymentProfile`, many-to-one, references; the profile exists before the pipeline and outlives it: relation `profile`; `ess/domains/policy.yaml`, entity `inference.policy.DeploymentProfile`.
- `Node → Port`, one-to-many, node owns port: entity `inference.graph.Node`, relation `ports`.
- `Edge → Port` twice, each references exactly one port: entity `inference.graph.Edge`, relations `source` (via `from_port`) and `sink` (via `to_port`).
- `Binding → Node`, references one: entity `inference.graph.Binding`, relation `node`. `Binding → Adapter`, references one, the adapter stands on its own: relation `adapter`; `ess/domains/model.yaml`, entity `inference.model.Adapter`, which references one `inference.model.Model` (relation `model`).
- `Port → Edge` (the inverse): zero to many on both ends. Neither `source` nor `sink` carries a uniqueness constraint, and fan-out and fan-in are composition patterns of the design (`docs/design/inference.md:539-540`). The per-port cardinality constraint of `docs/design/inference.md:480` (§9.2 item 6) has no field in graph.yaml; the epic that needs fan-in ordering (§23.3 test 3 is not in this epic) adds it.
- `Node → Binding` (the inverse): zero or one. Zero for a node that needs no adapter (`docs/design/inference.md:100`, a node can wrap a model or be deterministic; roles `source`, `transform`, `policy`, `fan_out`, `fan_in`, `sink` of `inference.graph.NodeRole`, read from lines 461-469); one where bound (command `inference.graph.PlanPipeline`, outcome `mismatch`, speaks of its bound adapter, singular).

## Acceptance

With the declaration commands and the typed builder in place, `cargo test -p inference-testkit --test conform` reports `inference.graph.DeclarePipeline/outcome/declared`, `inference.graph.AddPort/outcome/added`, `inference.graph.ConnectPorts/outcome/unbounded`, `inference.policy.DeclareProfile/outcome/declared`, `inference.graph.DeclarePipeline/outcome/unknown-profile`, `inference.graph.AddNode/outcome/unknown-pipeline`, `inference.graph.AddPort/outcome/unknown-node`, `inference.graph.ConnectPorts/outcome/unknown-pipeline`, `inference.graph.ConnectPorts/outcome/unknown-source`, `inference.graph.ConnectPorts/outcome/source-not-output`, `inference.graph.BindNode/outcome/unknown-pipeline`, `inference.policy.DeclareProfile/outcome/wrong-egress`, `inference.policy.DeclareProfile/outcome/remote-not-opted-in`, `inference.policy.DeclareProfile/outcome/offline-admits-cluster` as `passed` (today `unsupported`, each named in `ess/SKIPPED.md`) with none of the 14 left in that file, and the Rust test showing the builder and the commands yield the same declaration passes. The 9 ids under `## Scenarios that pass with story:pipeline-validation` arrange `inference.graph.ValidatePipeline`, which `story:pipeline-validation` answers; they stay in `ess/SKIPPED.md` naming it.

## Out of Scope

Validation and planning (`story:pipeline-validation`, `story:capability-planning`); runs; declarative TOML/YAML/JSON manifests (`docs/design/inference.md:491`, later per the epic); the audio formats and timebases a port will carry (`inference-audio`, `epic:contracts-and-testkit`).

## Ambiguities

- `inferable`: whether a declaration may change after validation. graph.yaml refuses `AddNode`, `ConnectPorts` and `BindNode` on a pipeline that is not Declared (`not-declared`), and declares no state guard on `AddPort` (its `UNMAPPED` note: the pipeline is two references away), so the runner records a port in every state; the typed builder consumes the declaration on validate (the Validated and Planned forms have no mutators), which is the compile-time help §9.3 names (`docs/design/inference.md:490`).
- `inferable`: the conformance verb. `ess verify conform run --target` on ess 0.60.0 admits only built-in targets (`billing`, `oracle-fixture`, `interpreted`); the runner in `inference-testkit` executes the suite itself and `ess verify conform report --suite --results` admits its results. The runner is created by `story:testkit-conformance-target`.

## Open Questions

None.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). No code exists in the tree yet: every path below is
read from the plan, not from a file.

- **Primary surface:** `crates/inference-pipeline` — cited; the body creates the crate (Context; `docs/design/inference.md:241`) and every later graph story is ordered behind it by `depends_on`
- **Files:** `crates/inference-testkit/tests/target/graph.rs` — cited, body Step 5; pre-created empty by `story:testkit-conformance-target` step 2
- **Files:** `crates/inference-testkit/tests/target/policy.rs` — cited, body Step 5; same
- **Files:** `crates/inference-testkit/Cargo.toml` — cited, body Context: the `inference-pipeline` dependency line enters the one `[dependencies]` table `story:tts-contract` also writes, unordered
- **Files:** `crates/inference-core/src/lib.rs` — cited: one `mod` line for the new `inference.policy` module; named as the one file shared with this story by `story:outbound-text-generations-and-segments` step 1 and `story:inbound-text-spans-and-envelope` step 3
- **Files:** `ess/SKIPPED.md` — cited, body Step 6: the eight ids leave the `## inference.graph` and `## inference.policy` sections (`story:testkit-conformance-target` step 3)
- **Symbols (new, in `inference-pipeline`):** `inference.graph.Pipeline` (`ess/domains/graph.yaml:140`), `Node` (209), `Port` (244), `Edge` (278), `Binding` (316); `PipelineId` (19), `PortId` (27), `EdgeId` (31), `BindingId` (35); `NodeRole` (49), `PortDirection` (54), `Essentiality` (59), `Delivery` (65), `InteractionMode` (70), `OverloadPolicy` (78); `DeclarePipeline` (582), `AddNode` (630), `AddPort` (684), `ConnectPorts` (745), `BindNode` (834); `UnboundedEdge` (435); `Edge.capacity` as `NonZeroUsize` (invariant `capacity > 0`, line 296) — cited
- **Symbols (new, in `inference-core`):** `inference.policy.ProfileId` (`ess/domains/policy.yaml:25`), `ProfileKind` (30), `Egress` (36), `DeploymentProfile` (49), `DeclareProfile` (88), `ProfileDeclared` (164) — cited
- **Symbols (answered by the testkit):** the five graph commands and `DeclareProfile`; the views `inference.graph.Pipelines` (graph.yaml:1415), `Nodes` (1438), `Ports` (1453), `Edges` (1478), `Bindings` (1493) and `inference.policy.Profiles` (policy.yaml:176), each queried by one of the eight scenarios in `.engineering/drafts/suite.json` — cited
- **Symbols (consumed, not defined here):** `inference.graph.NodeId`, `inference.graph.RunId`, `inference.model.AdapterId`, `Capability`, `Modality`, `Locality`, all in `inference-core` from `story:workspace-skeleton` — cited
- **Also likely:** `crates/inference-core/src/policy.rs` — inferred; the body says "a module of its own" and names no file
- **Also likely:** `crates/inference-testkit/src/graph.rs` — inferred; the body says "the `graph` and `policy` store modules under `src/`" and `story:testkit-conformance-target` step 1 pre-declares each as an empty file, but no body spells the name
- **Also likely:** `crates/inference-testkit/src/policy.rs` — inferred; same reading
- **Documents:** none; `ess/domains/graph.yaml` and `ess/domains/policy.yaml` are read, not written (body Step 1: "nothing to add"); `AGENTS.md`, `README.md`, `Taskfile.yml` and the root `Cargo.toml` (`members = ["crates/*"]`, `story:workspace-skeleton` step 1: no story of this epic edits the list) are untouched — cited
- **Confidence:** high — the body names every path down to the two per-domain target files, and the four neighbour bodies name the same shared files from their side; the only reading that could be wrong is the three file names under *also likely* — cited
- **Would collide with:** any unit adding a dependency or feature line to `crates/inference-testkit/Cargo.toml`; any unit adding a module to `crates/inference-core/src/lib.rs`; any unit removing ids from `ess/SKIPPED.md` (the `inference.tools` and `inference.session` domains are the ones no edge orders against this story); any unit writing the `graph` or `policy` domain of the testkit or `crates/inference-pipeline`, which the later graph stories do, each ordered behind this one by `depends_on` — cited
- **Safety fact:** every existing file this story writes is a slot `story:testkit-conformance-target` reserved per domain (`tests/target/graph.rs` and `policy.rs` answer `unsupported` until filled while `tests/target/mod.rs` dispatches by domain prefix; one `## <domain>` section each in `ess/SKIPPED.md`), and `conform.rs` asserts the `unsupported` set equals that file's ids, so filling the graph and policy slots changes no other domain's answer and a wrong removal is a red test rather than silent drift — step 1 (read from `story:testkit-conformance-target` steps 2 and 3; no code exists to point a `file:line` at), unproven — cited

## Scenarios that pass with story:pipeline-validation

Ids this story's commands answer whose arrangement also validates a pipeline, computed on 2026-10-19 as in `story:outbound-text-generations-and-segments`.

- `inference.graph.AddNode/outcome/added`
- `inference.graph.ConnectPorts/outcome/connected`
- `inference.graph.Edge/invariant/after/inference.graph.ConnectPorts/connected`
- `inference.graph.BindNode/outcome/bound`
- `inference.graph.AddNode/outcome/not-declared`
- `inference.graph.ConnectPorts/outcome/not-declared`
- `inference.graph.BindNode/outcome/unknown-node`
- `inference.graph.BindNode/outcome/unknown-adapter`
- `inference.graph.BindNode/outcome/not-declared`
