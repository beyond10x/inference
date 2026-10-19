---
format: aep.planning-md/3
id: story:text-pipeline-end-to-end
kind: story
status: active
title: A text-only pipeline of mock nodes runs to completion, cancels and drains (A-02)
summary: Source, mock LLM, sink on mocks; three runs; §23.3 test 7; the complete inference.graph suite
relations:
- decomposes: epic:graph-minimum
- depends_on: story:node-instance-lifecycle
- depends_on: story:bounded-edges
- depends_on: epic:contracts-and-testkit
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-core
- confidence: inferred
  path: crates/inference-core/src/envelope.rs
- confidence: cited
  path: crates/inference-runtime
- confidence: inferred
  path: crates/inference-runtime/Cargo.toml
- confidence: inferred
  path: crates/inference-runtime/src/lib.rs
- confidence: cited
  path: crates/inference-runtime/tests
- confidence: inferred
  path: crates/inference-runtime/tests/text_pipeline.rs
- confidence: cited
  path: crates/inference-testkit
- confidence: inferred
  path: crates/inference-testkit/src/graph.rs
- confidence: inferred
  path: crates/inference-testkit/src/llm.rs
revision: 20
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T11:45:47Z", actor: "human:timo", revision: 19, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "proposed", to: "active", at: "2026-10-19T11:45:47Z", actor: "human:timo", revision: 20, decided_on: {"recorded":{"review_outcome":1}}}
---
# Story: text-pipeline-end-to-end

## Outcome

A text-only pipeline of mock nodes, source to mock LLM to sink, is declared with the typed builder, validated, planned and run to Completed with every committed segment at the sink; a second run is cancelled while Active and delivers nothing after the cancel; a third is drained and completes with its in-flight segments delivered (A-02, `docs/design/inference.md:1138`).

## Context

Last story of `epic:graph-minimum`; it is the epic outcome. It wires the pieces of the six earlier stories into one integration test in `crates/inference-runtime/tests/` (per-crate tests, review finding F7 in `architecture-design:inference`; no example binary, because the repository ships libraries only).

The pipeline: a node with role `source` whose output port is `Text`, `essential`, `reliable`, `incremental: true`, `end_marker_required: true`, `provisional_allowed: false`, holding a fixed list of four text envelopes (`docs/design/inference.md:303-318`), numbered 1 to 4, and emitting the next one each time the test releases it, then the end-of-input control event from `inference-core` when the test releases the end; a node with role `model` requiring `output_streaming`, bound to the mock LLM adapter of `inference-testkit` (`story:llm-contract`, `epic:contracts-and-testkit`) through the `inference-llm` trait, which starts one `inference.stream.Generation` per run and, for each input envelope it receives, emits `inference.stream.TextDelta` then `inference.stream.TextSegmentCommitted` for one segment (`ess/domains/stream.yaml`, commands `EmitTextDelta` and `CommitSegment`), so segment `s<n>` answers input `n`; a node with role `sink` whose input port accepts `Text`, `essential`, `provisional_allowed: false`, collecting what it receives. The edge into the sink carries `overload: fail` (committed text is delivered or fails explicitly, line 685); the edge into the model carries `pause`.

Three runs on the same planned pipeline (`Run → Pipeline` is many-to-one):

1. to completion: the test releases inputs 1 to 4 and the end; the model completes its generation on the end-of-input event; the runtime loop issues `DrainRun` once the source has ended and `CompleteRun` once every instance has drained; the sink holds `s1, s2, s3, s4` in order;
2. cancelled: the test releases inputs 1 and 2, waits until the sink holds `s1, s2`, then issues `CancelRun` while the run is Active; the mock LLM is scripted for this run to emit one further segment after the cancel (computation may outlive cancellation of delivery, line 641); `CancelRun` cancels the run's Active generation and discards its unplayed segments, so that `EmitTextDelta` is refused `stale-generation`, no segment is created, and the sink never receives it (stale output is rejected at sinks, `CancelRun` summary in graph.yaml; `docs/design/inference.md:637`); the run ends Cancelled, the instances Stopped, the sink holds exactly `s1, s2`;
3. drained: the test releases inputs 1, 2 and 3 and issues `DrainRun` as soon as input 3 has been emitted, before the end and before input 4; no new input is admitted (a release after the drain emits nothing), `s3` is the in-flight segment and reaches the sink, the model commits what it has received and completes its generation on `drain`, the run ends Completed and the sink holds exactly `s1, s2, s3`.

§23.3 test 7 (graph runs drain and release worker resources, line 1130): after each run every node instance is Stopped, every edge queue is dropped and no task of the run is still alive, observed through weak references the test holds.

## Steps

1. Specification: nothing to add; `ess specify validate --path ess` stays valid.
2. The mock source and sink nodes in `inference-testkit` (the recording mock of `story:node-instance-lifecycle` extended with a released payload list and a collector); the model node wrapping the mock LLM adapter, scripted as above. If `inference-core` carries no end-of-input control event when this story starts (the contracts epic lists control events in its Scope and no story of it names this one), this story adds it to the control event family of `inference-core` (`docs/design/inference.md:297`).
3. The runtime loop that issues `DrainRun` once the source has ended and `CompleteRun` once every instance has drained, through the commands of `story:run-lifecycle`; when the test issues `CancelRun` (run 2) or `DrainRun` (run 3) itself, the loop only finishes the run from there and issues no command the state machine would refuse.
4. The three runs as one integration test; the resource check.
5. Closing check of the epic: the complete `inference.graph` suite, as re-synthesized by `story:node-instance-lifecycle` (`ess verify conform synthesize --path ess --scenarios ess/scenarios`), and every `inference.policy` scenario pass against the testkit runner (every earlier story claims its own scenarios; this is the re-run, not a new claim), and `ess/SKIPPED.md` names no `inference.graph` or `inference.policy` id (each earlier story removed its own; nothing is removed here).

## Domain relations

All `inferable`:

- `Run → Pipeline`, many-to-one: `ess/domains/graph.yaml`, entity `inference.graph.Run`, relation `pipeline` (three runs of one pipeline).
- `Generation → Run` and `Generation → Node`, references one each; `Generation → OutputSegment`, owns many: `ess/domains/stream.yaml`, entity `inference.stream.Generation`, relations `run`, `producer`, `segments`.
- `Run → Session`: none in this story; the envelope `session_id` is optional (`docs/design/inference.md:310`) and the relation is `UNMAPPED` in `ess/domains/session.yaml`, owned by `epic:session-facade`.

## Acceptance

The integration test in `inference-runtime` passes with the sink contents, end states and resource observations under Checks for each of the three runs, on a pipeline that before this story no test in the workspace declares, plans or runs end to end.

## Checks

1. Completed run: the sink holds exactly `s1, s2, s3, s4` in order and the run is Completed.
2. Cancelled run: the sink holds exactly `s1, s2`, the delta the mock emits after cancellation is refused `stale-generation` and creates no segment, the generation is Cancelled, and the run is Cancelled.
3. Drained run: the sink holds exactly `s1, s2, s3`, input 4 was never admitted, and the run is Completed.
4. After each run every node instance is Stopped and no run task or edge queue survives, observed through the weak references the test holds.
5. The complete `inference.graph` suite and every `inference.policy` scenario pass against the testkit runner, with `ess/SKIPPED.md` naming none of their ids.

## Out of Scope

Real adapters, audio, tools, sessions, subgraphs (epic Out of scope); fan-out and fan-in (§23.3 test 3); replay (test 8); the application-facing named source and sink interfaces of `docs/design/inference.md:511`, which have no noun in graph.yaml and are the mock source and sink nodes here until `epic:session-facade` declares one.

## Ambiguities

- `inferable`: the end of input is the control event family of `inference-core` (`docs/design/inference.md:297`: start, stop, commit, revise, cancel, error), carried on the same edge as the text (control and data may share a channel, line 169) and distinguishable by `kind` (line 314).
- Decision taken here, reversible in the test: the source emits on release by the test and the mock LLM commits one segment per input, so that the sink contents of each run are fixed before it starts; the drain point of run 3 is after input 3 is emitted and before input 4.

## Open Questions

None.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-runtime`, the integration test and the runtime loop that issues `DrainRun` and `CompleteRun` — cited (Context: "one integration test in `crates/inference-runtime/tests/`"; Acceptance: "the integration test in `inference-runtime`"; Steps 3-4; crate path `docs/design/inference.md:200,242`; per-crate `tests/` per review F7, `docs/design/review-2026-10-18.md:136-137`; `crates/` absent in the tree)
- **Files:** `crates/inference-runtime/tests`, the three runs and the §23.3 test 7 resource check as one test — cited (Context, Step 4, `docs/design/inference.md:1130,1138`)
- **Files:** `crates/inference-runtime/tests/text_pipeline.rs` — inferred: the body names the directory, not the file; three later stories put their own file "beside the A-02 test" (`story:endpoint-controller`, `story:tool-authority-and-executor-nodes`, `story:cascade-read-only-tool`) without naming it
- **Files:** `crates/inference-runtime/src/lib.rs` — inferred: the loop of Step 3 lands in the Run module `story:run-lifecycle` adds (file name not recorded) or in a module of its own, and only the second case adds a `mod` line
- **Files:** `crates/inference-runtime/Cargo.toml` — inferred: the test drives the testkit's mock LLM, which sits behind the testkit feature `llm` (`story:llm-contract` step 2), so the runtime's dev-dependency on `inference-testkit` gains that feature; no body names this edit
- **Files:** `crates/inference-testkit/src/graph.rs`, the mock source and sink nodes extending the recording mock of `story:node-instance-lifecycle` — inferred: Step 2 names `inference-testkit` and no file; `graph.rs` is the graph domain's module by the testkit layout (`story:testkit-conformance-target` step 1) and the recording mock's own file is not recorded anywhere
- **Files:** `crates/inference-testkit/src/llm.rs`, the model node wrapping the scripted mock LLM adapter — inferred: the mock generator lives there behind feature `llm` (`story:llm-contract` recorded scope); whether the node sits beside it or in `graph.rs` is this story's choice
- **Symbols:** `inference.graph.Run` relation `pipeline` (`ess/domains/graph.yaml:349,362`); `DrainRun`, `CompleteRun`, `CancelRun` (`graph.yaml:1139,1159,1224`; `CancelRun` summary line 1244); `NodeRole` variants `source`, `model`, `sink` (`graph.yaml:49-51`); port fields `role`, `essentiality`, `delivery`, `incremental`, `end_marker_required`, `provisional_allowed` and edge `overload` (`graph.yaml:220,259,262,263,265,267,293`); `inference.stream.Generation` relations `run`, `producer`, `segments` (`ess/domains/stream.yaml:129,143,152,160`); `EmitTextDelta`, `CommitSegment` (`stream.yaml:643,689`), events `TextDelta`, `TextSegmentCommitted` (`stream.yaml:884,895`); `inference.policy.DeclareProfile/outcome/declared` (`ess/domains/policy.yaml:88,145`); the `UNMAPPED` `Run → Session` relation (`ess/domains/session.yaml:50`) — cited
- **Also likely:** `crates/inference-core`, the end-of-input control event in the control family of `docs/design/inference.md:297` — cited (Step 2, conditional on `inference-core` not carrying one; the condition holds: `epic:contracts-and-testkit` Scope says "no separate control-event type is declared in `ess/` and none is built in this epic", and `story:inbound-text-spans-and-envelope` "declares no separate control-event type")
- **Also likely:** `crates/inference-core/src/envelope.rs` — inferred: the envelope's `kind` (`docs/design/inference.md:314`) is what tells control from content, and that module is where the inbound story's recorded scope puts the envelope
- **Not touched:** `ess/domains/*.yaml` and `ess/SKIPPED.md` — cited (Step 1 "nothing to add"; Step 5 "nothing is removed here", the suite is re-run, not re-claimed)
- **Not touched:** `crates/inference-pipeline` (typed builder, node and endpoint traits are used as built), `crates/inference-llm` (the trait is used, not changed), root `Cargo.toml`, `Taskfile.yml`, `AGENTS.md`, `README.md` — inferred from the Steps naming no change there
- **Documents:** none — cited (every step lands in Rust; `ess/` is validated, not edited)
- **Confidence:** medium — the three crates and `crates/inference-runtime/tests` are read from the body, but every file-level entry is inferred because neither this body nor the upstream bodies it extends name a file (the recording mock of `story:node-instance-lifecycle`, the Run module of `story:run-lifecycle`), and the tree holds no code to grep (no commits, `crates/` absent)
- **Would collide with:** any unit touching `crates/inference-runtime` (its `src/lib.rs` module list, its `Cargo.toml` dev-dependencies, a file in `tests/` beside this one); any unit editing the testkit's `src/graph.rs` or `src/llm.rs`; any unit adding to `inference-core`'s envelope or control-event family (`src/envelope.rs`) if the end-of-input event lands here; and, without sharing a file, any unit that changes the synthesized scenario set or `ess/SKIPPED.md`, because Step 5 re-runs the whole `inference.graph` suite and the `unsupported == ess/SKIPPED.md` equality in `crates/inference-testkit/tests/conform.rs`
- **Safety fact:** the story adds no `ess/` declaration and removes no `ess/SKIPPED.md` line (Steps 1 and 5), so the suite `ess verify conform synthesize --path ess` emits is unchanged and the `unsupported == ess/SKIPPED.md` assertion of `tests/conform.rs` (`story:testkit-conformance-target` step 2) cannot go red from this story; every code change is additive (a new test file, extensions to existing mocks, a loop over existing Run commands that issues nothing the state machine refuses) — step 1 (read from the body), unproven. Where the symbol search stops: the end-of-input control event; the envelope `kind` is declared in `ess/domains/stream.yaml` as a `types:` entry by the inbound story and mirrored in `crates/inference-core/src/envelope.rs`; if `kind` is a closed enum there, adding an end-of-input kind is an `ess/` edit Step 1 excludes (a `types:` entry synthesizes no scenario per the inbound story, so the equality holds either way) — step 1, unproven

## Envelopes on emitted events

Added 2026-10-19 from wave 5 (`review-result:adversary-inbound-text-spans-and-envelope-pass-1`, finding F2).

`story:inbound-text-spans-and-envelope` declared the event envelope (`inference.stream.EventEnvelope`, `crates/inference-core/src/envelope.rs`) and the testkit `EventLog` that refuses a duplicate event id, a non-increasing sequence per stream, a payload generation other than the envelope's and a kind other than the payload's declared name (`crates/inference-testkit/src/stream.rs`). The domain stores record bare events, because they do not know the producer node, port or stream. This story's runtime does: every event a node instance emits on a run is wrapped in an envelope (producer, port, stream, sequence per stream, timebase) and passes through the event log, so the rules of design §23.1 test 6 apply to every emitted event; a case asserts that a repeated `(stream_id, sequence)` from a node is refused.
