---
format: aep.planning-md/3
id: story:tts-contract
kind: story
status: implemented
title: 'TTS contract with a mock synthesiser: input completion and audio emission declared apart'
summary: inference-tts is the synthesis contract consuming committed OutputSegments and emitting audio frames; the testkit mock synthesiser keeps synthesised, queued and played apart and §23.1 test 3 reflects its declared semantics.
relations:
- decomposes: epic:contracts-and-testkit
- serves: vision:inference
- depends_on: story:audio-vocabulary
- depends_on: story:outbound-text-generations-and-segments
scope:
- confidence: cited
  path: crates/inference-testkit/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/src/tts.rs
- confidence: inferred
  path: crates/inference-testkit/tests/tts.rs
- confidence: cited
  path: crates/inference-tts
revision: 18
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T07:24:42Z", actor: "human:timo", revision: 15, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "proposed", to: "active", at: "2026-10-19T07:24:42Z", actor: "human:timo", revision: 16, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "active", to: "implemented", at: "2026-10-19T07:52:28Z", actor: "human:timo", revision: 18, decided_on: {"recorded":{"test_result":1,"review_outcome":6,"verification":1}}}
---
## Context

`docs/design/inference.md:378-390`, the TTS contract: input is committed or explicitly speculative text segments with synthesis options and a voice identity, output is audio frames or chunks with timing and lifecycle events where supported (lines 380-381); whether input may arrive incrementally or only as a complete utterance is declared (line 385); whether audio is emitted incrementally or only on synthesis completion is declared (line 386); synthesised, queued and played are distinct and only the client or media sink confirms playback (line 388); an uncommitted token stream is not safe to speak by default (line 390). The crate is `inference-tts` (line 196; line 238: depends on `core`, `text`, `audio`). The contract projects the declared nouns (answer to Q2 in `executable-system-specification:inference`): an implementation is an `inference.model.Adapter` declaring `incremental_input` (text before the utterance is complete) and `output_streaming` (audio before synthesis completes) separately (`inference.model.Capability`, lines 419-427); it consumes `inference.stream.OutputSegment`s through `QueueForSynthesis`, which the specification refuses for a Tentative segment (`ess/domains/stream.yaml`, command `inference.stream.QueueForSynthesis`, outcome `wrong-state`), and emits `inference-audio` frames; `AcknowledgePlayback` is issued by the sink, never by the synthesiser (lines 388 and 618).

Of the synthesis lifecycle events of line 336, `SynthesisCompleted` is declared: `inference.stream.CompleteSynthesis` moves a Queued segment to Synthesized (its audio exists and has not been heard, line 388) and emits it. `SynthesisStarted` is not declared; this story emits only declared events (`SegmentQueued`, `SynthesisCompleted`, `AudioPlayed`) and audio frames. If the implementor needs `SynthesisStarted`, the specification is extended first and the extension recorded in `executable-system-specification:inference`.

This story depends on `story:audio-vocabulary` for `inference-audio` and on `story:outbound-text-generations-and-segments` for `OutputSegment` and `Generation`, and on nothing of the inbound-text or STT stories: the mock yields its frames and segment transitions to the caller and records nothing in the testkit event log, so it touches no module of `story:inbound-text-spans-and-envelope` and runs beside it. With `story:llm-contract` and `story:audio-and-stt-contract` it shares two lines of `crates/inference-testkit/Cargo.toml`, each story's own optional dependency and its own feature line, pre-declared empty by `story:testkit-conformance-target`, and nothing else: each fills its own feature-gated module, so the three may merge in any order.

One more story shares a file with this one and is not ordered against it: `story:pipeline-declaration` (`epic:graph-minimum`) adds its `inference-pipeline` dependency line to `crates/inference-testkit/Cargo.toml`, the same table this story's optional `inference-tts` dependency line enters. The two lines are different entries of one table, each named in its body, and no other file is shared; the collision is named here and in `story:pipeline-declaration` rather than ordered, because an edge either way would hold a chain head behind an unrelated contract.

## Domain relations

- The mock synthesiser is an `inference.model.Adapter` referencing one `inference.model.Model`: inferable, `ess/domains/model.yaml`, entity `inference.model.Adapter`, relation `model`.
- The segments it consumes are `inference.stream.OutputSegment`s owned by a generation: inferable, `ess/domains/stream.yaml`, entity `inference.stream.Generation`, relation `segments`; a Queued segment moves to Synthesized when its audio is produced (`CompleteSynthesis`) and from Synthesized to Played only on the sink acknowledgement, and is then never discarded; a Synthesized segment not yet played is still discarded (`inference.stream.OutputSegment`, transitions `synthesize`, `play` and `discard`).

## Steps

1. `crates/inference-tts/`: the synthesis contract: declare the adapter record; accept committed segments one at a time or as a complete utterance according to the declared capability; produce `inference-audio` frames incrementally or at completion according to the other; move each segment whose audio is produced from Queued to Synthesized through `CompleteSynthesis`; hand a Tentative segment back with `SegmentStateConflict`, the `QueueForSynthesis` wrong-state outcome.
2. `inference-testkit`, feature `tts`: `crates/inference-testkit/Cargo.toml` gains the optional dependency on `inference-tts` and `tts = ["dep:inference-tts"]` in place of the `tts = []` the testkit story declared; the `tts` module, declared empty behind `#[cfg(feature = "tts")]` by that story, gains a scripted mock synthesiser in the four configurations of the two capabilities and a sink double that acknowledges playback of the frames it has been given, moving segments to Played.
3. §23.1 test 3 (line 1107) as Rust tests: for each configuration, the moment input is accepted (before or only after the utterance is complete) and the moment audio is emitted (before or only after synthesis completes) match the declared capabilities; a Tentative segment is refused; a segment is Queued before synthesis, Synthesized once its audio is produced and Played only after the sink acknowledges, a Queued segment refusing the acknowledgement; a Played segment refuses discard.

## Acceptance

`cargo test -p inference-testkit --features tts` passes with the §23.1 test 3 tests: each of the four mock configurations accepts input and emits audio exactly when its declared `incremental_input` and `output_streaming` say, a Tentative segment is refused with `SegmentStateConflict`, and a segment becomes Synthesized when its audio is produced, Played only on the sink acknowledgement after that, and then refuses discard.

## Out of scope

Voices, languages, codecs and speaker conditions (line 387) and text-to-audio alignment (line 389, `audio_text_alignment`), not declared in `ess/` and not needed by the exit of the epic, so `epic:local-tts-adapter` extends the specification for them; playback gating by output commitment as a graph policy (line 627, `epic:local-voice-cascade`); a real backend.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). No crate exists on disk yet: every `crates/` path is
read from this story's body or from the body of the story that pre-declares it, not from a file.

- **Primary surface:** `crates/inference-tts` — cited (Steps 1; `docs/design/inference.md:196`, `:238`), a new crate no other story of the epic writes
- **Files:** `crates/inference-testkit/Cargo.toml` — cited (Steps 2: the optional `inference-tts` dependency line and `tts = ["dep:inference-tts"]` replacing the `tts = []` that `story:testkit-conformance-target` step 1 pre-declares)
- **Files:** `crates/inference-testkit/src/tts.rs` — inferred; the body says "the `tts` module, declared empty behind `#[cfg(feature = "tts")]`" and names no filename (`src/tts/mod.rs` is the other reading)
- **Also likely:** `crates/inference-testkit/tests/tts.rs` — inferred; the Acceptance is `cargo test -p inference-testkit --features tts` and the body does not say whether §23.1 test 3 is an in-module `#[cfg(test)]` or an integration test file
- **Symbols:** `inference.stream.QueueForSynthesis` (`ess/domains/stream.yaml:718`, outcome `wrong-state` `:758-763`), `inference.stream.SegmentStateConflict` (`:267`), `inference.stream.AcknowledgePlayback` (`:768`), `inference.stream.SegmentQueued` (`:902`), `inference.stream.AudioPlayed` (`:912`), `inference.stream.OutputSegment` transitions `queue`/`play`/`discard` (`:222-233`), `inference.model.Capability` variants `output_streaming`/`incremental_input` (`ess/domains/model.yaml:31-32`), `inference.model.Adapter` relation `model` (`:145-151`) — cited
- **Not touched:** root `Cargo.toml` (`members = ["crates/*"]`, `story:workspace-skeleton` step 1), `crates/inference-testkit/src/lib.rs` (`story:testkit-conformance-target` step 1: no later story adds a line to it), `Taskfile.yml`, `AGENTS.md`, `ess/SKIPPED.md`, `crates/inference-core`, `crates/inference-text`, `crates/inference-audio` (consumed through `depends_on`, not written) — cited
- **Documents:** none — cited; conditional only: if the implementor needs `SynthesisStarted`/`SynthesisCompleted` (`docs/design/inference.md:336`, absent from `ess/`), the body orders an extension of `ess/domains/stream.yaml` and a record in `executable-system-specification:inference` first
- **Confidence:** high — the story names the crate, the manifest lines and the module; only the two filenames inside the testkit are read into it
- **Would collide with:** any unit editing the `[dependencies]` or `[features]` table of `crates/inference-testkit/Cargo.toml`; any unit writing under `crates/inference-tts/`; any unit touching the testkit's `tts` feature module
- **Safety fact:** the mock yields frames and segment transitions to its caller and records nothing in the testkit event log, so it shares no module with the stream, STT or LLM stories, and the three contract stories meet only in two lines each of one manifest — cited (Context, paragraph 3; `story:testkit-conformance-target` step 1), level 2, unproven: the pre-declared `stt = []`, `llm = []`, `tts = []` lines are adjacent and each story's optional-dependency line has no named insertion point, so a three-way merge can report a textual conflict on a semantically disjoint change

## Frame length

Decided in wave 8 by the implementor of this story: `inference-audio` does not refuse a frame whose `data` length disagrees with `sample_count * channels * width(encoding)`. `ess/domains/audio.yaml` cannot state the invariant, so the serde wire keeps admitting what the specification admits (existing pinned cases deserialize such frames, e.g. `crates/inference-audio/tests/adversary2_bytes_and_narrowing.rs`, two samples with one byte of data). `inference-audio` gains `AudioEncoding::width` and `AudioFormat::data_len(sample_count)`. The producing contract refuses instead: `inference_tts::SynthesisOutput::audio` refuses a mismatched frame with `FrameLengthMismatch`, so every synthesiser, not only the mock, emits well-formed frames. Cases: `crates/inference-testkit/tests/tts.rs` `every_frame_carries_sample_count_times_channels_times_encoding_width_bytes` (both encodings, one and two channels, four configurations) and `crates/inference-tts/tests/synthesis_output.rs` `a_frame_whose_data_length_disagrees_with_its_samples_is_refused`.
