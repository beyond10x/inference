---
format: aep.planning-md/3
id: story:node-instance-lifecycle
kind: story
status: draft
title: 'Node-instance lifecycle: describe, prepare, start, drain or cancel, stop, declared in the specification first'
summary: Extends inference.graph.NodeInstance in ess/domains/graph.yaml, the node-instance trait, the recording mock, the run drives its instances
relations:
- decomposes: epic:graph-minimum
- depends_on: story:run-lifecycle
- depends_on: story:bounded-edges
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-pipeline
- confidence: inferred
  path: crates/inference-pipeline/src/lib.rs
- confidence: cited
  path: crates/inference-runtime
- confidence: inferred
  path: crates/inference-runtime/Cargo.toml
- confidence: inferred
  path: crates/inference-runtime/src/lib.rs
- confidence: inferred
  path: crates/inference-runtime/tests
- confidence: cited
  path: crates/inference-testkit
- confidence: inferred
  path: crates/inference-testkit/src/graph.rs
- confidence: inferred
  path: crates/inference-testkit/src/lib.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/graph.rs
- confidence: cited
  path: ess/domains/graph.yaml
revision: 16
---
# Story: node-instance-lifecycle

## Outcome

Each node of a running pipeline participates as a node instance that is described, prepared, started, drained or cancelled, and stopped by the run that owns it, with a failed instance surfacing a typed error that fails the run, so a node implementation has one trait to write and the runtime drives it.

## Context

Sixth story of `epic:graph-minimum`. The design gives nodes a lifecycle akin to describe, prepare, start, exchange, drain or cancel, stop (`docs/design/inference.md:673`) and an asynchronous open, exchange events, drain or cancel, close shape (line 484). `inference.graph.NodeInstance` (`ess/domains/graph.yaml`) is declared with its first state only, Described, by decision D6 of `executable-system-specification:inference`, to grow with this epic. The runtime owns the lifecycle of node instances (line 655) and consumes node implementations via interfaces (line 160); the ESS entity is the node descriptor and the Rust trait its projection (`executable-system-specification:inference`, answer to Q2).

## Step 1 is the specification

Extend `ess/domains/graph.yaml` before any code:

- `inference.graph.NodeInstance.lifecycle`: states `Described`, `Prepared`, `Started`, `Draining`, `Cancelling`, `Stopped`, `Failed`; terminal `Stopped` and `Failed`; transitions `prepare` Described to Prepared, `start` Prepared to Started, `drain` Started to Draining, `cancel` from Described, Prepared, Started, Draining to Cancelling, `stop` from Prepared, Draining, Cancelling to Stopped, `fail` from Described, Prepared, Started, Draining to Failed. The names are the verbs of line 673; `exchange` is what a Started instance does (events flow along the edges, `ActivateRun` summary), not a state. `fail` and `cancel` leave Described because `prepare` is issued on a Described instance and may return an error, and a run cancelled or failed while Validating holds Described instances that must still reach a terminal state.
- commands `PrepareNodeInstance`, `StartNodeInstance`, `DrainNodeInstance`, `CancelNodeInstance`, `StopNodeInstance`, `FailNodeInstance { reason }`, each taking `instance_id`, each with its moving outcome, its event (`NodeInstancePrepared`, `NodeInstanceStarted`, `NodeInstanceDraining`, `NodeInstanceCancelling`, `NodeInstanceStopped`, `NodeInstanceFailed { reason }`) and a `wrong-state` outcome with a new error `inference.graph.NodeInstanceStateConflict { current: inference.graph.NodeInstance.State }`, the shape every Run command already has.
- the view `inference.graph.NodeInstances` gains `state`.

Then `ess specify validate --path ess` (valid) and `ess verify conform synthesize --path ess --out .engineering/drafts/suite.json` (0 refusals); the new scenario ids are read from that file, not guessed, and named in the PR. Say in the PR that this step came first.

## How the run drives its instances

Each coupling is read from a command summary in graph.yaml: `ValidateRun` (validating its plan against the resources at hand) describes one instance per node of the pipeline; `PrepareRun` (node instances are being prepared and resources admitted) prepares each; `ActivateRun` (events flow along the edges) starts each with its edge endpoints from `story:bounded-edges`; `DrainRun` (no new input is admitted, in-flight events complete) drains each; `CompleteRun` (drained and released its worker resources) stops each; `CancelRun` and `FailRun` cancel then stop each instance that is not already terminal. An instance whose prepare, start or exchange returns an error moves to Failed and the runtime issues `FailRun` with that reason (`docs/design/inference.md:1006`; D10).

This coupling belongs to the Rust runtime and is exercised by the Rust tests. In the conformance runner each command emits only the event graph.yaml declares for it, so a Run command moves no instance and emits no `NodeInstance*` event; the 66 Run scenarios of `story:run-lifecycle` assert `expect_no_event` for every other event and keep passing after the re-synthesis.

## Steps

1. Specification, as above.
2. Contract: the node-instance trait in `inference-pipeline` (the crate that owns nodes, `docs/design/inference.md:241`; node implementations such as the VAD, turn-policy, tool-authority and executor nodes of `epic:tools-and-turns` land in that crate too, and `inference-runtime` depends on it, line 242, so the trait cannot live in the runtime without a cycle) with `describe`, `prepare`, `start`, `drain`, `cancel`, `stop`, each asynchronous and executor-neutral in the public signature (line 665), `start` receiving the node's input and output endpoints as trait objects of the `InputEndpoint` and `OutputEndpoint` traits that `story:bounded-edges` declares in `inference-core` (module `endpoint`, beside the envelope) and implements on the receiver and sender sides of its queue in `inference-runtime`, so the node trait names no runtime type and `inference-pipeline` keeps `core` and the port types as its only dependencies; the runtime drives the trait and owns the instance state machine (line 655).
3. Behaviour: the state machine and the coupling above; a `wrong-state` refusal moves nothing.
4. Testkit: a recording mock node in `inference-testkit` that implements the trait, records every call and fails on demand; the runner answers the six new commands and the extended view. The scenarios step 1 adds were synthesized after `ess/SKIPPED.md` was written, so that file names none of them; this story answers every one of them rather than listing any, and `cargo test -p inference-testkit --test conform` (which asserts `unsupported` equals that file) stays green.
5. Tests: the re-synthesized `inference.graph.NodeInstance` scenarios and the three Rust tests under Checks.

## Domain relations

All `inferable`, from `ess/domains/graph.yaml`:

- `Run → NodeInstance`, one-to-many, owns, one instance per node of the pipeline per run: relation `instances`; the describe step at `ValidateRun` and the rest as read above from the command summaries.
- `NodeInstance → Node`, references one: relation `node`; the instance reaches its adapter through `Pipeline.bindings`, `Binding.node` and `Binding.adapter` (D3), never through a field of its own.
- `NodeInstance → Run` failure coupling: an instance in Failed makes the runtime issue `inference.graph.FailRun` (read from line 1006; D10).

## Acceptance

With `ess/domains/graph.yaml` extended as in step 1, so that `ess specify validate --path ess` reports `valid` and the file declares the six commands `PrepareNodeInstance`, `StartNodeInstance`, `DrainNodeInstance`, `CancelNodeInstance`, `StopNodeInstance` and `FailNodeInstance`, the error `inference.graph.NodeInstanceStateConflict` and the field `state` on the view `inference.graph.NodeInstances` (none of which it declares today, where `inference.graph.NodeInstance` has the one state Described), every scenario the re-synthesized suite derives for `inference.graph.NodeInstance` and those six commands passes against the testkit runner and the three Rust tests under Checks pass.

## Checks

1. A run of two recording mock nodes driven to Completed produced the call sequence describe, prepare, start, drain, stop on each node.
2. A run driven to Cancelled from Active produced describe, prepare, start, cancel, stop on each node.
3. A run whose node fails on start ends Failed with the reason the node returned, that instance in Failed and every other instance in Stopped.

## Out of Scope

Data flowing through `start` (`story:text-pipeline-end-to-end`); retries and health checks (line 661); per-execution versus per-session node state (`docs/design/inference.md:1230`, Q5, open in the design).

## Ambiguities

- `inferable`: at which run transition instances are described: `ValidateRun`, because describe precedes prepare (line 673) and `PrepareRun` is where graph.yaml says instances are being prepared.
- `inferable`: `cancel` leads to `stop` rather than to a terminal state of its own, because line 673 reads drain/cancel then stop.
- Decision taken here, reversible in `ess/domains/graph.yaml`: the state and command names of step 1, participles and verbs of line 673, and `cancel` and `fail` leaving Described.

## Open Questions

None.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). No crate exists in the tree yet (`git ls-files` is
empty; the tree holds `docs/`, `ess/`, `.engineering/` and the root skeleton), so every `crates/`
path below is one a sibling story promises to create, at the layout `story:workspace-skeleton`
step 1 fixes (`members = ["crates/*"]`).

- **Primary surface:** `crates/inference-runtime` — cited; the body says the runtime drives the trait, owns the instance state machine and the run-to-instance coupling ("This coupling belongs to the Rust runtime"), the path from `story:bounded-edges` Context ("creates `crates/inference-runtime`")
- **Files:** `ess/domains/graph.yaml` — cited; step 1 extends `inference.graph.NodeInstance.lifecycle` (today `states: [Described]`, lines 353-356), adds an error under `errors:` (358), six commands under `commands:` (386), six events under `events:` (815) and `state` on the view `inference.graph.NodeInstances` (1013-1021)
- **Files:** `crates/inference-pipeline` — cited; step 2 puts the node-instance trait in `inference-pipeline`, the path from `story:pipeline-declaration` Context ("creates `crates/inference-pipeline`")
- **Files:** `crates/inference-testkit` — cited; step 4 puts the recording mock node in `inference-testkit`, the path from `story:testkit-conformance-target` step 1
- **Files:** `crates/inference-testkit/tests/target/graph.rs` — cited; step 4 has the runner answer the six new commands and the extended view, and `story:testkit-conformance-target` step 2 fixes one target file per domain (`tests/target/graph.rs` for `inference.graph`) so that stories do not meet in `conform.rs` or `mod.rs`
- **Symbols:** `inference.graph.NodeInstance`, `inference.graph.NodeInstanceStateConflict`, `inference.graph.NodeInstances`, `PrepareNodeInstance`, `StartNodeInstance`, `DrainNodeInstance`, `CancelNodeInstance`, `StopNodeInstance`, `FailNodeInstance`, the trait methods `describe`, `prepare`, `start`, `drain`, `cancel`, `stop` — cited; none exists outside `ess/domains/graph.yaml` today (`DescribeNodeInstance` at 793, `NodeInstanceDescribed` at 913 are the only `NodeInstance` declarations beyond the entity)
- **Also likely:** `crates/inference-pipeline/src/lib.rs` — inferred; a new trait module needs its `mod` line, the body names no module
- **Also likely:** `crates/inference-runtime/src/lib.rs` — inferred; same reason, by analogy with `story:run-lifecycle` ("adds a module of its own and one line to `src/lib.rs`")
- **Also likely:** `crates/inference-runtime/tests` — inferred; the body does not place the three Rust tests under Checks, `story:text-pipeline-end-to-end` puts its runtime integration test in `crates/inference-runtime/tests/`
- **Also likely:** `crates/inference-runtime/Cargo.toml` — inferred; tests that drive the recording mock node need `inference-testkit` as a dev-dependency, which `story:bounded-edges` (which creates the file with `inference-pipeline` and `inference-core` only) does not add
- **Also likely:** `crates/inference-testkit/src/graph.rs` — inferred; the `inference.graph` store module of `story:testkit-conformance-target` step 1, where the instance state the extended view reports would be kept
- **Also likely:** `crates/inference-testkit/src/lib.rs` — inferred; the testkit's `lib.rs` declares one store module per domain plus `stt`, `llm`, `tts` and nothing for a mock node, so a recording mock node is either a new module with a `mod` line here or lives in `src/graph.rs`
- **Not this story's:** `crates/inference-core` (the body names it only as where `story:bounded-edges` declares `InputEndpoint` and `OutputEndpoint`, module `endpoint`), `ess/SKIPPED.md` (step 4: "that file names none of them; this story answers every one of them rather than listing any"), `AGENTS.md`, `Taskfile.yml`, the root `Cargo.toml` (`story:workspace-skeleton` step 1: no graph-minimum story edits the members list), `docs/` — cited
- **Documents:** `ess/domains/graph.yaml` is the specification document and changes first (title: "declared in the specification first"); `.engineering/drafts/suite.json` is regenerated by `ess verify conform synthesize` but git-ignored (`.gitignore` line 2), so it is no merge surface and carries no scope entry — cited
- **Confidence:** medium — the body names every crate and the one spec file (that part is high), but no crate exists, so every path below crate level is read from sibling stories' layout promises, not from code
- **Would collide with:** any unit editing `ess/domains/graph.yaml`; any unit adding a module or trait to `crates/inference-pipeline` (the body itself says the `epic:tools-and-turns` node implementations land there and will implement this trait); any unit touching the Run state machine or `src/lib.rs` of `crates/inference-runtime`; any unit filling `crates/inference-testkit/tests/target/graph.rs` or `src/graph.rs`, or adding a `mod` line to `crates/inference-testkit/src/lib.rs` (`story:pipeline-declaration` Context: "the one line all of them share")
- **Safety fact:** in the conformance runner a Run command emits only the event graph.yaml declares for it, so adding six `NodeInstance*` events leaves the 66 Run scenarios of `story:run-lifecycle` green: today's `inference.graph.PrepareRun/outcome/preparing` in `.engineering/drafts/suite.json` already asserts `expect_no_event` for `inference.graph.NodeInstanceDescribed` and every other event of the system (41 `expect_no_event` steps), and the re-synthesis will add the six new ones to that list; the one run-lifecycle assertion that does change is `inference.graph.DescribeNodeInstance/outcome/described`, whose `expect_view` on `inference.graph.NodeInstances` (today `contains` `instance_id`, `node_id`, `run_id`) gains `state` once the view declares it — step 2 (pointed at the file), unproven against the re-synthesized suite, which does not exist yet
