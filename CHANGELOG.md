# Changelog

## [Unreleased]

### Added

- `inference-facade`: `SessionEvent::UsageRecorded`, and usage totals per run and per session
  (`LiveSession::usage`, `UsageTotal` in `inference-core`), where an unknown count is never added as
  0; `LiveSession::record_inputs` records a session's ingress inputs for replay.
- `inference-facade`, feature `cost`: a usage record priced against a caller's price document
  (`CostRecord`), with `price_unknown` lines instead of zero and a session cost total through
  `LiveSession::price_with`; the synthesized suite grows from 460 to 462 scenarios, all answered.
- A subgraph node's `usage` output port also carries the usage of its child run, counted once under
  the parent run.
- The documentation site moves from mdBook under `site/` to Docusaurus under `website/`, with
  pages generated from the source by the `inference-docs` crate and checked by `task check`.

### Changed

- `inference-llm`: a cancelled or superseded generation keeps the usage its turn reported, as a
  final or partial record, instead of dropping it.
- `inference-llama-server` and `inference-llm` move to `llm` 0.8.3; the adapter uses its
  `AnonymousResolver`, and a server response without a usage chunk reports its tokens from the
  server's timings.

### Fixed

- `inference-openai`: the GPT-Live binding no longer claims `interruptible`. A caller can neither
  cancel nor interrupt its responses, so nothing stops its work within a declared latency.
- A nested subgraph drain no longer loses input sent right before stop.
- An output port of a subgraph node with no edge no longer fails the run; the child's output on it
  is read and dropped.
- A subgraph node's egress bridges end with the child run, so a child that keeps its endpoint past
  stop no longer hangs the drain, and a drain fails when every parent edge of a port refused.

## [0.7.0] — 2026-10-21

### Added

- `inference-duplex`: the session ledger records one usage record per native speech response once
  its usage is settled, one per reported transcription, and holds usage no response owns yet as a
  session record before the session closes or fails, so billed usage is not lost at the end.
  OpenAI Realtime, GPT-Live (billed session seconds) and Nova Sonic report through it; a count the
  provider leaves out is unknown, never 0.
- `inference-stt`: a recognition ends with `TranscriptEvent::Usage`, after its last committed span:
  input audio counted exactly from the frames offered, one request, and the backend's local compute
  where the adapter measures it (sherpa-onnx and whisper.cpp do); a failed call is recorded with
  `final: false`. The testkit STT node sends it on a `usage` port.
- `inference-tts`: a synthesis ends with `SynthesisEvent::Usage`: characters taken, output audio,
  one request and local compute where measured; the backend-neutral TTS checks check it, and the
  testkit TTS node sends it on a `usage` port.
- `inference-openai`: `SessionConfig::speed` sets the Realtime output speed (0.25 to 1.5).
- `examples/repo-podcast`: Claude distils a repository, two voice models discuss it over the
  speaker in a terminal UI with running token and cost counters.
- `examples/local-voice-cascade`: `INFERENCE_CASCADE_STT` (or `talk --stt`) selects the sherpa-onnx
  or the whisper.cpp recogniser by configuration; the same cascade tests pass under both.

### Changed

- Breaking: `TranscriptEvent::span_id` and `SynthesisEvent::segment_id` return an `Option`, since
  the usage event names no span or segment; after a recognition's or synthesis's usage record,
  further moves are refused (`RecognitionEnded`, `SynthesisEnded`), and the record is refused while
  a span is uncommitted or a segment is not synthesized, unless the call failed.
- The local cascade's `stt` node requires no capability, so a batch recogniser binds to it;
  `STREAMING_STT_REQUIRES` keeps the stricter declaration.
- CI, `ess` and the testkit's `ess-conformance` and `ess-primitives` move from 0.60.0 to 0.66.0; the
  specification is unchanged and the synthesized suite stays at 460 scenarios, all answered; the
  testkit target reads the inputs 0.66.0 synthesizes with optional fields left out.

## [0.6.0] — 2026-10-20

### Added

- `adapters/inference-sherpa-onnx`: the sherpa-onnx streaming recogniser as a local STT adapter
  (incremental input, output streaming), linked against the shared library built without TTS so
  no GPL phonemiser is linked; checkpoint and native archive pinned by sha256; conformance tests
  and a capability record for the reference machine.
- `adapters/inference-whispercpp`: whisper.cpp as a local batch STT adapter (no streaming claims),
  CPU-only; silence and non-speech annotations commit nothing.
- `adapters/inference-llama-server`: a local llama.cpp `llama-server` as an output-streaming LLM
  through `llm`'s Chat Completions client: cancellation, failures and deadlines, tool proposals as
  structured events, and one capability record per configuration.
- `adapters/inference-openai`: OpenAI GPT-Live (`v1/live/sessions`) behind feature `gpt-live`, as
  an `inference-duplex` native speech model that the probe observes as native duplex.
- `inference.model.CapabilityRecord`: an adapter's claims, the tests that evidence them, languages,
  formats and a resource profile per hardware target; the testkit checks a record file against
  the adapter's declaration.
- `inference.usage`: one usage record per model call (who, when, quantities, each reported,
  measured or unknown); `inference-llm` emits one per `llm` turn on a pipeline's `usage` egress.
- `inference-pipeline`: the commit-gate node; planning refuses an adapter whose locality the
  deployment profile does not admit, inside subgraphs too.
- `inference-runtime`: the egress watch that fails an `offline_strict` run on a connection or
  unconnected datagram socket that is not loopback; a recorder and replay driver that compare a
  run's control decisions; a best-effort telemetry sink that never delays a node.
- `inference-audio-device`: a sink that acknowledges playback and drops frames of superseded
  generations after a barge-in.
- `examples/local-voice-cascade`: STT, LLM and TTS through commit gates with barge-in, on the
  testkit mocks and, behind feature `local`, on sherpa-onnx and `llama-server` (TTS still a mock);
  `examples/stt-sherpa-onnx`; `realtime-voice` shows usage per response and session totals,
  selects `openai`, `openai-live` or `nova` by `--provider`, and its `compare` example measures
  latency and usage across providers.
- `inference-testkit`: backend-neutral TTS adapter checks.
- The documentation site under `site/`, deployed to GitHub Pages on each release tag.

### Changed

- The specification adds the capability record and the usage domain (`ess verify diff` against
  0.5.0: 18 changes, all compatible); the synthesized suite grows from 454 to 460 scenarios, all
  answered.
- `AGENTS.md` states the licence policy for local adapters and that tests download no weights and
  start no model server.
- CI installs `libclang-dev` and `cmake` and gives the check job 45 minutes.

## [0.5.0] — 2026-10-20

### Added

- `inference-facade`: the session facade. An application opens one session, sends, receives,
  commits input, interrupts, describes and closes it, and sees one event vocabulary whether the
  session runs the text-only pipeline or the voice cascade; the topology switches by
  configuration; history and a live session over a running graph.
- `inference-duplex`: the native speech session contract (one multiport node for a model that
  listens and speaks), its session ledger, a mock, and a conformance probe that reports whether a
  model's native duplex claim was observed.
- `inference-pipeline`: exposed ports, declarative graph definitions validated against runtime
  node descriptors, and subgraphs that keep cancellation and tracing boundaries; the endpoint
  controller, voice activity and turn policy nodes; tool retries by side-effect class with an
  at-most-once start; a bounded tool feedback loop through the LLM node; tool calls under
  interruption.
- `adapters/inference-openai`: OpenAI Realtime as an `inference-duplex` native speech model, with
  offline tests and live tests gated on a credential (environment, or the local keychain behind a
  feature).
- `adapters/inference-aws`: Amazon Nova Sonic as an `inference-duplex` native speech model, with
  offline and live tests; service errors are shown with ARNs and account ids redacted.
- `adapters/inference-audio-device`: microphone capture and speaker playout over cpal, with a
  lock-free ring and G.711 conversion.
- `examples/realtime-voice`: talk to a realtime model from a laptop
  (`cargo run -p realtime-voice --example talk -- --provider openai|nova`).
- `inference-testkit`: the session to run link and the session store; the conformance target
  answers all 454 scenarios of the synthesized suite.

### Changed

- `inference-llm` is a thin wrapper over the `llm` repository's model contract
  (`llm_core::Model`, tag 0.8.0). A `Generator` refuses an adapter claim its model does not
  declare; a turn that showed no delta gives the outcome's text as one segment. The workspace's
  `rust-version` moves from 1.88 to 1.98, which that tag requires.
- The workspace includes `adapters/*`.
- The specification adds the session, duplex and audio-encoding changes above (`ess verify diff`
  against 0.4.0: 146 changes; 3 breaking for readers, the new `AudioEncoding` variants `pcm_mulaw`
  and `pcm_alaw` and the new `GraphDefect` variant `duplicate_exposure_name`; 23 unknown, 120
  compatible). The synthesized suite grows from 317 to 454 scenarios.

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
