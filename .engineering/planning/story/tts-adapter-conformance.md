---
format: aep.planning-md/3
id: story:tts-adapter-conformance
kind: story
status: implemented
title: Backend-neutral TTS adapter checks in the testkit
summary: inference-testkit checks any Synthesiser with a sink in the loop against the TTS adapter exit criteria; shown on the mock and on two doubles that over-claim.
relations:
- decomposes: epic:local-tts-adapter
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T08:23:42Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-20T08:23:42Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-20T13:52:16Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Context

`epic:local-tts-adapter` exits on checks that must pass with the adapter and a sink in the loop:
§23.1 test 3, input completion and audio emission semantics reflected in the record
(`docs/design/inference.md:1107`); the scenarios
`inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis` and
`inference.stream.OutputSegment/transition/play/by/inference.stream.AcknowledgePlayback/played`;
and §23.2 lines 1114-1117 (claims match observed behaviour; errors, cancellation and finalisation
propagate; a disconnect leaks no task and revives no stale output; audio timestamps and format
transformations are valid). Today they run only on the testkit mock
(`crates/inference-testkit/tests/tts.rs`); nothing an adapter crate can call runs them against its
own `Synthesiser`.

The contract refuses a frame of a Queued segment without the `output_streaming` claim and input
before the end of the utterance without `incremental_input` (`crates/inference-tts/src/lib.rs:68-78`).
It does not refuse a claim the synthesiser never exercises; that direction is what these checks add.

The first concrete synthesiser, sherpa-onnx offline TTS, waits on
`decision-blocker:sherpa-tts-gpl-phonemiser`. These checks do not depend on the backend, and
`epic:backend-replacement` can run them on the second TTS backend unchanged.

## Planned scope

- A module `inference_testkit::tts::conformance` behind the existing `tts` feature
  (`crates/inference-testkit/Cargo.toml:15`), generic over `inference_tts::Synthesiser`, with the
  existing `MockSink` (`crates/inference-testkit/src/tts.rs:392`) as the sink. No contract crate
  changes and no new `ess/` noun.
- One public check per exit item, each returning what it observed or a typed failure naming the
  item:
  - tentative refused: a Tentative segment offered is refused with `SegmentStateConflict` and no
    audio of it is emitted;
  - played only by the sink: a Committed segment moves Queued, Synthesized, Played, and reaches
    Played only through `acknowledge_playback` with the receipt the sink made from every frame of
    the segment;
  - claims observed: with input offered segment by segment, audio of a segment arrives before
    `end_utterance` exactly when `incremental_input` is claimed, and a frame of a segment arrives
    while it is Queued exactly when `output_streaming` is claimed; a claim not observed fails;
  - audio valid: every frame of a synthesis has one format (equal to the expected one when the
    caller passes it), a data length equal to `AudioFormat::data_len`, and positions contiguous
    and increasing on the synthesis sample clock;
  - cancellation: a segment discarded while Queued yields no further frame, and a handle dropped
    mid-utterance releases its concurrency slot, so the next `start` succeeds.
- The text the checks speak is a parameter (default: two short English sentences), so a real
  backend is driven with words.
- Tests in `crates/inference-testkit/tests/`: every check passes on `MockSynthesiser` in each of
  its four `MockConfiguration`s; a double that claims `output_streaming` but emits audio only at
  completion fails the claims check, and so does one that claims `incremental_input` but takes
  nothing before the utterance ends.

## Domain relations

- Synthesis -> OutputSegment, one-to-many, a segment in at most one synthesis; the synthesis owns
  the segment record from Queued to Played or Discarded, and a segment exists before it is offered
  - inferable (inferred from `crates/inference-tts/src/lib.rs:10-12` and `:27-29`; no ess/1
  document declares a synthesis entity).
- PlaybackReceipt -> OutputSegment, one receipt per segment, made by the sink from every frame of
  it - inferable (inferred from `crates/inference-tts/src/lib.rs:17-22` and `:224-225`).
- OutputSegment -> Run, many-to-one, via `run_id` - inferable from `ess/domains/stream.yaml`,
  entity `inference.stream.OutputSegment`, relation `run`.
- Adapter -> Model, many-to-one, via `model_id` - inferable from `ess/domains/model.yaml`, entity
  `inference.model.Adapter`, relation `model`. The checks read claims from the adapter record only.

## Acceptance

`cargo test -p inference-testkit --features tts` passes, with every check accepting
`MockSynthesiser` in all four configurations and the claims check rejecting each of the two lying
doubles with a failure that names the capability it did not observe.

## Out of scope

The sherpa-onnx adapter crate, its checkpoint and its checkpoint-gated test (behind
`decision-blocker:sherpa-tts-gpl-phonemiser`); text-to-audio alignment, which the contract does not
carry yet (`crates/inference-tts/src/lib.rs:141-143`); STT checks.
