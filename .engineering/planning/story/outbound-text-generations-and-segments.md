---
format: aep.planning-md/3
id: story:outbound-text-generations-and-segments
kind: story
status: draft
title: 'Generations and output segments: tentative, committed, queued, played'
summary: inference-core gains Generation and inference-text gains OutputSegment with their lifecycles; the target answers the 46 generation and segment scenarios.
relations:
- decomposes: epic:contracts-and-testkit
- depends_on: story:testkit-conformance-target
- serves: vision:inference
scope:
- confidence: inferred
  path: crates/inference-core/src/generation.rs
- confidence: cited
  path: crates/inference-core/src/lib.rs
- confidence: inferred
  path: crates/inference-testkit/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/src/stream.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/stream.rs
- confidence: cited
  path: crates/inference-text
- confidence: cited
  path: ess/SKIPPED.md
revision: 10
---
## Context

Outbound text, `docs/design/inference.md:610-620`: generated or tentative text, committed output segments, queued-for-synthesis text, audio generated but not yet played, played audio confirmed by a sink acknowledgement; speech already played is irreversible (line 620). The default commit rules of lines 622-628: synthesis consumes committed segments (line 626), cancellation suppresses stale unplayed media and uncommitted output (line 628). On interruption the current generation epoch is marked superseded (line 634) and stale results from it are rejected at irreversible sinks (line 637); cancellation of delivery is distinct from cancellation of computation (line 641).

The nouns are `ess/domains/stream.yaml`: `inference.stream.Generation` (Active, Completed, Superseded, Cancelled) and `inference.stream.OutputSegment` (Tentative, Committed, Queued, Played, Discarded); the commands `StartGeneration`, `CompleteGeneration`, `SupersedeGeneration`, `CancelGeneration`, `EmitTextDelta`, `CommitSegment`, `QueueForSynthesis`, `AcknowledgePlayback` and `DiscardSegment`; the errors `GenerationStateConflict` and `SegmentStateConflict`; the nine events from `GenerationStarted` to `SegmentDiscarded`; the views `Generations` and `Segments`. Crates follow the answer to Q1 in `executable-system-specification:inference`: `Generation` and `inference.stream.GenerationId` in `inference-core`, `OutputSegment` and `inference.stream.SegmentId` in `inference-text` (`docs/design/inference.md:192`; line 234: `inference-text` depends on `core` only). This story creates `crates/inference-text/`.

## Domain relations

- `inference.stream.Generation` references `inference.graph.Run`, cardinality one. Inferable: `ess/domains/stream.yaml`, entity `inference.stream.Generation`, relation `run`. The scenarios pass a literal `run_id` and create no run (`.engineering/drafts/suite.json`, `inference.stream.StartGeneration/outcome/started`), so the store accepts any `RunId` value.
- `inference.stream.Generation` references `inference.graph.Node`, cardinality one, as its producer. Inferable: `ess/domains/stream.yaml`, entity `inference.stream.Generation`, relation `producer` (via `node_id`). No Node exists in this epic (its Out of scope); the producer is a `NodeId` value the caller supplies.
- `inference.stream.Generation` owns `inference.stream.OutputSegment`, cardinality many, zero allowed; a segment has no meaning outside its generation, and cancelling the generation discards its unplayed segments. Inferable: `ess/domains/stream.yaml`, entity `inference.stream.Generation`, relation `segments` (via `generation_id`); decision D2 in `executable-system-specification:inference`. The cascade itself, one supersede or cancel discarding every unplayed segment of the generation, is not in this story: the specification artifact records it as a set effect covered by an authored scenario in `epic:local-voice-cascade`.

## Steps

1. `inference-core`: `Generation`, in a module of its own, with its state and the transitions `complete`, `supersede` and `cancel` as methods returning the moved entity and its event, or `GenerationStateConflict` carrying the current state. `story:pipeline-declaration` adds the `inference.policy` types to `inference-core` in a module of its own and runs after this story (its `depends_on` edge to `story:inbound-text-spans-and-envelope` orders it behind both stream stories); the one file both touch is `crates/inference-core/src/lib.rs`, each adding its own `mod` line.
2. `crates/inference-text/`: `OutputSegment` with its five states and the transitions `commit`, `queue`, `play` and `discard` the same way; `discard` moves from Tentative, Committed or Queued only, so a Played segment answers `SegmentStateConflict` (line 620), and a Tentative segment refuses `queue` (line 626, line 390).
3. `inference-testkit`: the `stream` store module and `tests/target/stream.rs`, both declared empty by `story:testkit-conformance-target`, gain stores for generations and segments, the two views with their `state` field and the nine commands answered by the target; the `## inference.stream` section of `ess/SKIPPED.md` loses exactly the 46 ids listed below. `story:pipeline-declaration` fills the `graph` and `policy` modules and the `## inference.graph` and `## inference.policy` sections, and `story:tool-call-lifecycle` the `tools` ones, so this story meets neither in a module nor in a section of `ess/SKIPPED.md`.

## Acceptance

`cargo test -p inference-testkit --test conform` passes with the 46 ids listed under `## Scenarios` in the report's `passed`, among them `inference.stream.Generation/transition/supersede/by/inference.stream.SupersedeGeneration/superseded`, `inference.stream.CancelGeneration/outcome/cancelled`, `inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis`, `inference.stream.OutputSegment/state/Played/refuses/inference.stream.DiscardSegment` and `inference.stream.OutputSegment/transition/play/by/inference.stream.AcknowledgePlayback/played`, with `failed` and `error` at 0 and none of the 46 ids left in `ess/SKIPPED.md`.

## Out of scope

Turns, spans and the event envelope (next story); any mock; the set effect of supersede or cancel on segments (`epic:local-voice-cascade`).

## Scenarios

The 46 ids this story moves from `ess/SKIPPED.md` to `passed`, read from `.engineering/drafts/suite.json` on 2026-10-18:

- `inference.stream.AcknowledgePlayback/outcome/played`
- `inference.stream.AcknowledgePlayback/outcome/wrong-state`
- `inference.stream.CancelGeneration/outcome/cancelled`
- `inference.stream.CancelGeneration/outcome/wrong-state`
- `inference.stream.CommitSegment/outcome/committed`
- `inference.stream.CommitSegment/outcome/wrong-state`
- `inference.stream.CompleteGeneration/outcome/completed`
- `inference.stream.CompleteGeneration/outcome/wrong-state`
- `inference.stream.DiscardSegment/outcome/discarded`
- `inference.stream.DiscardSegment/outcome/wrong-state`
- `inference.stream.EmitTextDelta/outcome/emitted`
- `inference.stream.Generation/state/Cancelled/refuses/inference.stream.CancelGeneration`
- `inference.stream.Generation/state/Cancelled/refuses/inference.stream.CompleteGeneration`
- `inference.stream.Generation/state/Cancelled/refuses/inference.stream.SupersedeGeneration`
- `inference.stream.Generation/state/Completed/refuses/inference.stream.CancelGeneration`
- `inference.stream.Generation/state/Completed/refuses/inference.stream.CompleteGeneration`
- `inference.stream.Generation/state/Completed/refuses/inference.stream.SupersedeGeneration`
- `inference.stream.Generation/state/Superseded/refuses/inference.stream.CancelGeneration`
- `inference.stream.Generation/state/Superseded/refuses/inference.stream.CompleteGeneration`
- `inference.stream.Generation/state/Superseded/refuses/inference.stream.SupersedeGeneration`
- `inference.stream.Generation/transition/cancel/by/inference.stream.CancelGeneration/cancelled`
- `inference.stream.Generation/transition/complete/by/inference.stream.CompleteGeneration/completed`
- `inference.stream.Generation/transition/supersede/by/inference.stream.SupersedeGeneration/superseded`
- `inference.stream.OutputSegment/state/Committed/refuses/inference.stream.AcknowledgePlayback`
- `inference.stream.OutputSegment/state/Committed/refuses/inference.stream.CommitSegment`
- `inference.stream.OutputSegment/state/Discarded/refuses/inference.stream.AcknowledgePlayback`
- `inference.stream.OutputSegment/state/Discarded/refuses/inference.stream.CommitSegment`
- `inference.stream.OutputSegment/state/Discarded/refuses/inference.stream.DiscardSegment`
- `inference.stream.OutputSegment/state/Discarded/refuses/inference.stream.QueueForSynthesis`
- `inference.stream.OutputSegment/state/Played/refuses/inference.stream.AcknowledgePlayback`
- `inference.stream.OutputSegment/state/Played/refuses/inference.stream.CommitSegment`
- `inference.stream.OutputSegment/state/Played/refuses/inference.stream.DiscardSegment`
- `inference.stream.OutputSegment/state/Played/refuses/inference.stream.QueueForSynthesis`
- `inference.stream.OutputSegment/state/Queued/refuses/inference.stream.CommitSegment`
- `inference.stream.OutputSegment/state/Queued/refuses/inference.stream.QueueForSynthesis`
- `inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.AcknowledgePlayback`
- `inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis`
- `inference.stream.OutputSegment/transition/commit/by/inference.stream.CommitSegment/committed`
- `inference.stream.OutputSegment/transition/discard/by/inference.stream.DiscardSegment/discarded`
- `inference.stream.OutputSegment/transition/play/by/inference.stream.AcknowledgePlayback/played`
- `inference.stream.OutputSegment/transition/queue/by/inference.stream.QueueForSynthesis/queued`
- `inference.stream.QueueForSynthesis/outcome/queued`
- `inference.stream.QueueForSynthesis/outcome/wrong-state`
- `inference.stream.StartGeneration/outcome/started`
- `inference.stream.SupersedeGeneration/outcome/superseded`
- `inference.stream.SupersedeGeneration/outcome/wrong-state`

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-text` — cited, created by this story (Context: "This story creates `crates/inference-text/`"; `docs/design/inference.md:192`, `:234`)
- **Files:** `crates/inference-core/src/lib.rs` (one `mod` line), `crates/inference-testkit/tests/target/stream.rs`, `ess/SKIPPED.md` (`## inference.stream` section, minus the 46 listed ids) — cited, Steps 1 and 3; the testkit path is composed with `story:testkit-conformance-target` step 2
- **Symbols:** `Generation`, `OutputSegment`, `GenerationStateConflict`, `SegmentStateConflict` created; `GenerationId`, `SegmentId`, `RunId`, `NodeId` consumed from `inference-core` (`story:workspace-skeleton`) — cited, `ess/domains/stream.yaml:121,172,223,229,34,38`
- **Also likely:** `crates/inference-testkit/src/stream.rs` — inferred, the file behind "the `stream` store module" (`story:testkit-conformance-target` step 1 says one empty file per domain and names no path; `src/stream/mod.rs` is the other reading)
- **Also likely:** `crates/inference-core/src/generation.rs` — inferred, the name of the "module of its own" in Step 1; no other unit touches it under any name
- **Also likely:** `crates/inference-testkit/Cargo.toml` — inferred, a `[dependencies]` line on `inference-text` so the stream store can hold `OutputSegment`; no body names it, and the testkit story's manifest pulls `inference-core` only
- **Documents:** `ess/SKIPPED.md` only — cited; `ess/domains/stream.yaml` is read, not changed (all 26 names the story builds are already declared); root `Cargo.toml` is not edited (`members = ["crates/*"]`, `story:workspace-skeleton` step 1)
- **Confidence:** high — the story and its `depends_on` neighbour name every file and the crate split follows Q1 of `executable-system-specification:inference`; none of the files exists on disk yet, so the names are the plan's, not the tree's
- **Would collide with:** any unit filling the testkit's `inference.stream` store or target module; any unit adding a `mod` line to `crates/inference-core/src/lib.rs`; any unit editing `ess/SKIPPED.md` (the bodies keep per-domain sections apart, `waves` sees one file); any unit adding a line to the `[dependencies]` table of `crates/inference-testkit/Cargo.toml`, the one surface shared with a unit that is not ordered against this story
- **Safety fact:** the story declares nothing new in `ess/`: its 26 names are already in `ess/domains/stream.yaml` (ids 34, 38; entities 121, 172; errors 223, 229; commands 366-561; events 620-672; views 708, 723) and its 46 ids are 46 of the 60 `inference.stream` ids in `.engineering/drafts/suite.json`, so the suite stays at 153 and `ess/SKIPPED.md` shrinks by exactly 46 lines, guarded by the conform test's `unsupported == SKIPPED` assertion; outside `stream.yaml` these types are named only by `ess/domains/tools.yaml:81,96,149,203,228`, through `GenerationId`, which already lives in `inference-core` — step 2 (`file:line`), unproven: no test runs on this tree yet
