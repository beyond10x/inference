# Changelog

## [0.4.0] — 2026-10-19

### Added

- `inference-stt`: the STT contract with no concrete model, the scripted mock recogniser and the
  `stt-only` example, whose dependency tree carries no runtime.
- `inference-tts`: the TTS contract, the mock synthesiser and a sink double; playback is
  acknowledged with a `PlaybackReceipt` built from the synthesized audio.
- `inference-pipeline`: pipelines declared through commands or the typed builder, checked against
  a catalog of declared models and adapters, validated with the structural checks of design 10.2
  and planned from the weakest node's capabilities; the node-instance trait; the tool authority
  node, which validates arguments against the tool's schema, binds the tool's
  `requires_confirmation` through a confirmation hook that confirms or denies, and decides each
  call once; the tool executor node, generic over the executor contract, which runs each call
  once under its deadline.
- `inference-runtime`: bounded edges with backpressure and the overload policies; the `Run`
  state machine and its nine commands; the run controller, which drives node instances through
  prepare, start, drain and stop; `RunController::drive`, which ends a run when every source has
  sent end of input or on a drain or cancel request, draining nodes upstream first; a guard that
  refuses a repeated or decreasing sequence among the content events a node emits on a stream.
- `inference-tools`: the `ToolCall` lifecycle (propose, authorize, reject, start, complete, fail),
  the executor contract and the provisional-input barrier.
- `inference-core`: the control event kinds (`ControlKind`; end of input is `stop`) and
  `SendError::SequenceNotIncreasing`.
- `inference-testkit`: the run stores behind an optional `run` feature; mock source, sink and
  model nodes of a pipeline; the scripted mock tool executor with its invocation log. The
  conformance target answers 296 of 317 scenarios, every `inference.graph`, `inference.stream`,
  `inference.policy` and `inference.tools` scenario among them.

### Changed

- `OutputSegment` transitions take the segment by value, so a segment cannot be committed twice;
  `Generator::start` returns a `Result`, and the LLM mock refuses a second concurrent generation.
- The specification adds the node-instance and tool-call lifecycles, their events and errors, and
  `AddPort.pipeline_id` (`ess verify diff` against 0.3.0: 67 changes; 12 breaking, all new state
  variants for readers and the new required `AddPort` input for callers; 27 unknown, 28
  compatible). The synthesized suite grows from 229 to 317 scenarios.

### Fixed

- The defects of the 0.3.0 review in the runtime queue (every discard counted, coalesce keeps the
  newest, stale-epoch sends wake), the pipeline (forced outcomes, ports under their pipeline) and
  the STT and TTS contracts.

### Known issues

- The 21 `inference.session` scenarios are unsupported until the session facade lands; they are
  named in `ess/SKIPPED.md`.
- A run waits for a cancel when a node that consumes events fails, and a cancel arriving between a
  tool node's check and its send can let one event through; both are recorded on the tool-call
  interruption story.
- `ess verify conform synthesize` still refuses the three `affects` cascades (ESS-SYNTH-001); the
  authored scenarios execute them.

## [0.3.0] — 2026-10-19

### Added

- `inference-core` follows the 0.2.0 model vocabulary (`orchestrated_duplex`, a model's
  `provenance` and `source_revision`, `ModelNotFound`) and gains `Model::declare_as` and
  `Adapter::declare_as` for stores that assign identifiers, `Generation` with its lifecycle, and the
  event envelope (`EventEnvelope`, `Timebase`, `EventId`).
- `inference-audio`: audio frames, formats and sample clock positions, declared first as the
  `inference.audio` domain in `ess/domains/audio.yaml`; frame data is canonical base64 on the wire.
- `inference-text`: output segments (tentative, committed, queued, synthesized, played, discarded),
  turns and spans; a committed span is immutable.
- `inference-llm`: the LLM contract, with no concrete model. Deltas stream only where the adapter
  claims `output_streaming`, and input is taken during output only where it claims
  `concurrent_input_output`; admitted input is bound to the generation that admitted it.
- `inference-testkit`: in-memory stores per domain, the event log, a conformance target driven by
  the `ess-conformance` runner whose results `ess verify conform report` admits, and the mock
  generator behind the `llm` feature. `ess/SKIPPED.md` names every scenario the target does not
  answer yet and the story that will.
- `task check` builds and tests with `--all-features`.

### Changed

- The specification adds the `inference.audio` domain and the event envelope types to
  `inference.stream` (`ess verify diff` against 0.2.0: 8 changes, all compatible); the synthesized
  suite is unchanged at 229 scenarios.

### Known issues

- The testkit target answers 15 of the 229 scenarios; the other 214, most of which arrange a run,
  are named in `ess/SKIPPED.md` with the story that will answer them.
- `ess verify conform synthesize` still refuses the three `affects` cascades (ESS-SYNTH-001); the
  authored scenarios execute them against the interpreted target.
- The conformance results the testkit writes carry a fixed `completed_at`, not the run's own time.

## [0.2.0] — 2026-10-19

### Added

- The Cargo workspace under `crates/` and its first crate, `inference-core`: the `inference.model`
  vocabulary of `ess/domains/model.yaml` (models, adapters, capabilities, modalities, locality,
  execution form, the concurrency refusal) and the shared identifiers of runs, nodes, streams,
  generations and sessions, read only in the canonical Uuid spelling.
- `task check` runs `cargo fmt`, `cargo clippy -D warnings` and `cargo test` after the
  specification and planning checks.
- `ess/scenarios/`: four authored scenarios for what ess 0.60.0 synthesis does not reach: the
  discard cascades of supersede, cancel and run cancellation (ESS-SYNTH-001), and a Draining run
  that fails.

### Changed

- The specification fixes 66 findings of a hardening pass and five reviews (13 P1): a superseded
  or cancelled generation emits, commits, queues and plays nothing more; the tool barrier reads
  the tool's class and the span's state instead of the caller's claim; profiles are checked
  against the design's table; runs, sessions and graph edits refuse the wrong pipeline or run
  state; refusals carry typed diagnostics. Breaking against 0.1.0 (`ess verify diff`, exit 4):
  new enum members, `FailRun.failure: FailureReason` replacing `reason`, new required inputs on
  `DeclareModel`, `DeclareTool` and `ProposeToolCall`, the new `DeclareProfile` refusal.

### Known issues

- `ess verify conform synthesize` yields 229 scenarios with 3 ESS-SYNTH-001 refusals, the
  `affects` cascades of `CancelRun`, `CancelGeneration` and `SupersedeGeneration`; the authored
  scenarios execute them and the interpreted run passes 229 of 229.

## [0.1.0] — 2026-10-19

The design, its specification and its plan. No code yet.

### Added

- `docs/design/inference.md`, the design (v0.2): vocabulary, contracts, streaming semantics, the
  graph, the runtime, the roadmap and the open questions; `docs/design/review-2026-10-18.md`, its
  review, with the owner's decisions on each finding.
- `ess/`, the ESS specification: `system.yaml`, `ess-inputs.yaml` and six domain documents
  (model, policy, graph, stream, tools, session) with 17 entities and 36 commands, every
  declaration citing the design line it was read from; `ess verify conform synthesize` yields
  153 scenarios with 0 refusals.
- `.engineering/planning`, the AEP planning store: a vision, product requirements, an
  architecture design, the ESS record, 12 epics, 22 stories with typed scope in 18 waves, 4
  decision blockers and 24 review results from two critic rounds.
- `Taskfile.yml` with `task check`; `.github/workflows/check.yml` runs it with pinned `ess`
  0.60.0 and `aep` 0.74.0 releases; `.github/workflows/shared-gates.yml` calls the common source
  gates at an exact commit.
