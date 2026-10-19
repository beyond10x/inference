---
format: aep.planning-md/3
id: story:bounded-edges
kind: story
status: active
title: 'Bounded edges: finite buffering, backpressure and the five overload policies'
summary: One bounded queue per edge per run; pause, fail, drop, coalesce, drop_stale_epochs; §23.3 test 4
relations:
- decomposes: epic:graph-minimum
- serves: vision:inference
- depends_on: story:pipeline-declaration
- depends_on: story:inbound-text-spans-and-envelope
- depends_on: story:outbound-text-generations-and-segments
scope:
- confidence: inferred
  path: Cargo.lock
- confidence: cited
  path: crates/inference-core
- confidence: inferred
  path: crates/inference-core/src/endpoint.rs
- confidence: inferred
  path: crates/inference-core/src/lib.rs
- confidence: cited
  path: crates/inference-runtime
- confidence: cited
  path: crates/inference-runtime/Cargo.toml
- confidence: cited
  path: crates/inference-runtime/src/lib.rs
- confidence: inferred
  path: crates/inference-runtime/src/queue.rs
revision: 20
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T07:24:41Z", actor: "human:timo", revision: 19, decided_on: {"recorded":{"review_outcome":3}}}
- {from: "proposed", to: "active", at: "2026-10-19T07:24:42Z", actor: "human:timo", revision: 20, decided_on: {"recorded":{"review_outcome":3}}}
---
# Story: bounded-edges

## Outcome

Every edge of a run is a bounded queue with the capacity and overload policy its declaration carries, so a slow consumer never grows memory without bound and each of the five overload behaviours `pause`, `fail`, `drop`, `coalesce` and `drop_stale_epochs` of `inference.graph.OverloadPolicy` is observable in a test; the two further variants, `cancel` and `supersede`, are refused when a queue is built.

## Context

Fifth story of `epic:graph-minimum`; it creates `crates/inference-runtime` (graph execution and operational policies; depends on `inference-pipeline`, `inference-core` and runtime utilities, `docs/design/inference.md:242`) with the queue as its first module. Every edge specifies finite buffering and overload behaviour (`docs/design/inference.md:679`); typed event routing happens across bounded queues (line 658). `inference.graph.Edge` (`ess/domains/graph.yaml`) carries `capacity` (invariant `capacity > 0`, decision D9 of `executable-system-specification:inference`) and `overload`, an `inference.graph.OverloadPolicy` whose variants name the behaviours of the §12.3 table (lines 679-689; decision D5): `pause`, `cancel`, `drop`, `coalesce`, `supersede`, `fail`, `drop_stale_epochs`. This story implements five of them and refuses `cancel` (live audio, line 683) and `supersede` (STT hypotheses, line 684) at construction with a typed error, because the table names them without saying what a full queue does under them. The item carried is the event envelope of §6.2 (`docs/design/inference.md:303-318`), the `inference-core` type `story:inbound-text-spans-and-envelope` declares in `ess/` and lands; its `generation_id` (line 315) names an `inference.stream.Generation` (`ess/domains/stream.yaml`), the `inference-core` type of `story:outbound-text-generations-and-segments`, with an integer `epoch`. The envelope carries no epoch of its own, so the queue reads it through a lookup from `inference.stream.GenerationId` to the `epoch` of the Generation it names, handed to the queue at construction (the shape the planner's adapter lookup has in `story:capability-planning`); the tests hand it a map, `story:text-pipeline-end-to-end` hands it the generations the model node started. An item whose envelope carries no `generation_id` (a control event, line 297) is never stale.

This story depends on `story:pipeline-declaration` for `inference.graph.Edge` and `inference.graph.OverloadPolicy` in `inference-pipeline`, on `story:inbound-text-spans-and-envelope` for the envelope and on `story:outbound-text-generations-and-segments` for `Generation.epoch`. It does not depend on `story:run-lifecycle`: its tests read nothing a run produces, so it runs beside `story:pipeline-validation` and `story:capability-planning`, and `story:run-lifecycle` depends on it for the crate files (`Cargo.toml`, `src/lib.rs`) it creates.

One queue exists per edge per run: a Run is one execution of a planned pipeline (graph.yaml, `inference.graph.Run` summary), the edge declares the bound (line 679), and the runtime instantiates the queue when the run is prepared (`story:node-instance-lifecycle` wires that; this story delivers the queue).

The behaviours, each read from its table row:

- `pause` (line 683): when the queue holds `capacity` items, the producer waits until the consumer takes one; nothing is lost.
- `fail` (line 685, committed text: deliver or return an explicit failure, never silently discard): when full, the send returns a typed error to the producer carrying the refused item; the node surfaces it to the run controller (line 1006), which issues `FailRun` (`story:node-instance-lifecycle` wires that; this story returns the error).
- `drop` (line 683, drop only under explicit policy): when full, the arriving item is discarded and the producer is not blocked; the count of discarded items is readable on the queue.
- `coalesce` (line 684, coalesce or supersede old hypotheses; line 686, buffer or coalesce tentative deltas): when full, the arriving item replaces the newest queued item that carries the same `stream_id` (line 311), or is discarded if none does; a replaced item counts as discarded.
- `drop_stale_epochs` (line 688, drop stale epochs on interruption): the queue remembers the highest `epoch` seen per producer node (envelope `producer`, line 313; `Generation.producer`); an arriving item whose generation has a lower epoch is discarded, and when a higher epoch arrives the queued items of lower epochs are discarded first.

The last three are decisions this story takes where the table leaves the mechanism open (which item `drop` and `coalesce` give up, how an epoch is compared); each is reversible in `inference-runtime` and bound to the line it reads.

## Steps

1. Specification: nothing to add; `ess specify validate --path ess` stays valid. No scenario names a queue (`.engineering/drafts/suite.json` holds none for `inference.graph.Edge` beyond the declaration ones), so `ess/SKIPPED.md` loses nothing here.
2. Crate: `crates/inference-runtime/` with its `Cargo.toml` (dependencies `inference-pipeline`, `inference-core`) and `src/lib.rs` naming the queue module; `story:run-lifecycle` adds its module afterwards.
3. Contract: two endpoint traits, `InputEndpoint` and `OutputEndpoint`, declared in `inference-core` (module `endpoint`, beside the envelope they carry; `inference-core` is "minimal shared primitives and semantics", line 233, and both `inference-pipeline` and `inference-runtime` depend on it, lines 241-242), so that a node implementation and the node trait of `story:node-instance-lifecycle` can name an endpoint without `inference-pipeline` depending on the runtime, and so that this story writes nothing under `crates/inference-pipeline` and stays beside `story:pipeline-validation` and `story:capability-planning` (the `mod endpoint;` line lands in `crates/inference-core/src/lib.rs`, a crate this story is already ordered behind through the inbound and outbound stories); then the queue type in the `inference-runtime` module, constructed from an `inference.graph.Edge` and the epoch lookup, whose receiver side implements `InputEndpoint` and whose sender side implements `OutputEndpoint`; the send error type for `fail`; a readable discard count; executor-neutral in its public signature (line 665).
4. Behaviour: the five policies above; the queue never holds more than `capacity` items under any policy; building a queue from an edge whose overload is `cancel` or `supersede` returns the typed construction error.
5. Tests (§23.3 test 4, backpressure remains bounded under slow consumers, `docs/design/inference.md:1127`): the five under Checks, one per policy, each over a queue built from an `inference.graph.Edge` carrying that policy.

## Domain relations

All `inferable`:

- `Edge → queue in a run`: `inference.graph.Edge` fields `capacity` and `overload` (`ess/domains/graph.yaml`), `inference.graph.Run` summary (one execution of a planned pipeline), `docs/design/inference.md:679` and `658`.
- `Generation → Run` and `Generation → Node`, each references one; `Generation.epoch` is an Integer: `ess/domains/stream.yaml`, entity `inference.stream.Generation`, relations `run` and `producer`.
- `envelope → Generation`, zero or one (`generation_id`, `docs/design/inference.md:315`, "if any"), as `story:inbound-text-spans-and-envelope` declares it; the queue resolves it through the lookup above and treats an item without one as never stale.

## Acceptance

The five bounded-queue tests under Checks pass in `inference-runtime`, each over a queue built from an `inference.graph.Edge` carrying the policy it names, where before this story no queue type exists in the workspace.

## Checks

With `capacity` written `c`, items numbered in arrival order and the discard count read from the queue:

1. `pause`: a producer of 1000 items and a consumer that takes one item per tick; the queue length is at most `c` at every tick, all 1000 items reach the consumer in order, the discard count is 0.
2. `fail`: `c` items sent and none consumed; the send of item `c + 1` returns the typed error carrying item `c + 1`, and the consumer then reads items 1 to `c`.
3. `drop`: a burst of `2c` items and no consumer; the consumer then reads items 1 to `c` in order and the discard count is `c`.
4. `coalesce`: a burst of `2c` items on one `stream_id` and no consumer; the consumer then reads items 1 to `c - 1` in order followed by item `2c`, and the discard count is `c` (items `c` to `2c - 1`, each replaced by its successor); one further item on a `stream_id` no queued item carries, arriving while the queue is full, is discarded and raises the count to `c + 1`.
5. `drop_stale_epochs`: two items of a generation with epoch 1 queued, then one item of a generation with epoch 2 from the same producer arrives; the consumer reads only the epoch 2 item and the discard count is 2; a later item of epoch 1 is discarded on arrival and raises the count to 3.

## Out of Scope

A separate best-effort telemetry channel (line 689); the `Backpressure` runtime event family (line 340); jitter management for live audio (line 683); wiring the `fail` error to `FailRun` (`story:node-instance-lifecycle`, `story:text-pipeline-end-to-end`); the Run state machine (`story:run-lifecycle`); the `cancel` and `supersede` behaviours, refused at construction until a story reads them from lines 683-684.

## Ambiguities

- Decisions taken, listed in Context with the table row each reads; none is a relation, so none is `requires-stakeholder-input`, and each is reversible in one file.
- Decision taken here: the epoch reaches the queue through a lookup from `inference.stream.GenerationId`, because the envelope (`docs/design/inference.md:303-318`) has no epoch field and `story:inbound-text-spans-and-envelope` declares it without one; reversible in `inference-runtime` if the envelope ever gains the field.

## Open Questions

None.

## Scope

Adjusted 2026-10-19 by the coordinator after the scoper ran: the endpoint traits moved from `inference-pipeline` to `inference-core` (bounded-edges step 3, node-instance-lifecycle step 2), so every `crates/inference-pipeline` path below reads `crates/inference-core`, a crate `story:workspace-skeleton` creates and this story is already ordered behind; the store entries carry the corrected paths.

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). No code exists in the tree; every path below is a
path the story or a `depends_on` story creates.

- **Primary surface:** `crates/inference-runtime` — cited, the story creates the crate (Context; step 2; `docs/design/inference.md:242`)
- **Files:** `crates/inference-runtime/Cargo.toml`, `crates/inference-runtime/src/lib.rs` — cited, step 2; `story:run-lifecycle` Context names the same two files as the reason for its `depends_on` edge
- **Secondary surface:** `crates/inference-pipeline` — cited, step 3 declares `InputEndpoint` and `OutputEndpoint` in module `endpoint` of that crate (created by `story:pipeline-declaration`; `story:node-instance-lifecycle` step 2 reads them from there)
- **Symbols:** `InputEndpoint`, `OutputEndpoint` (new, in no file yet), `inference.graph.Edge` fields `capacity`, `overload` (`ess/domains/graph.yaml:278-296`), `inference.graph.OverloadPolicy` (`ess/domains/graph.yaml:78-80`), `inference.stream.Generation.epoch` and relation `producer` (`ess/domains/stream.yaml:129-156`), `inference.stream.GenerationId` — cited
- **Also likely:** `crates/inference-runtime/src/queue.rs` — inferred, the body says "the queue module" and never names the file
- **Also likely:** `crates/inference-pipeline/src/endpoint.rs` — inferred, the default file of the module the story names; the crate layout is `story:pipeline-declaration`'s, unlanded
- **Also likely:** `crates/inference-pipeline/src/lib.rs` — inferred, the `mod endpoint;` line a new module needs; the body does not say it
- **Also likely:** `Cargo.lock` — inferred, a new crate with dependencies regenerates it if it is committed; `.gitignore` does not exclude it and no story says
- **Documents:** none — `ess/` unchanged (step 1; `.engineering/drafts/suite.json` holds one `inference.graph.Edge` scenario, the `ConnectPorts` invariant), `ess/SKIPPED.md` unchanged, `docs/design/inference.md` read only, root `Cargo.toml` untouched because it globs `crates/*` (`story:workspace-skeleton` step 1) — cited
- **Confidence:** high — the story names the crate it creates, its two files and the module it adds to `inference-pipeline`; only file names inside those two crates and the lock file are inferred
- **Would collide with:** any unit adding a module to `crates/inference-pipeline` (its `src/lib.rs` `mod` line) or changing `Edge` or `OverloadPolicy` there; any unit writing under `crates/inference-runtime/`; any unit regenerating `Cargo.lock`. The two stories the body says run beside this one both write `crates/inference-pipeline`
- **Safety fact:** the queue reads only `Edge.capacity`, `Edge.overload` and `OverloadPolicy` from `inference-pipeline` and adds one new module there, while the stories beside it change `IncompatibleGraph`, `CapabilityMismatch` and the Planned form, none of which the queue reads; the root manifest admits the new crate by glob — step 2 (`story:pipeline-validation` steps 1-2, `story:capability-planning` steps 1-2, `story:workspace-skeleton` step 1), unproven: none of those files exists yet
