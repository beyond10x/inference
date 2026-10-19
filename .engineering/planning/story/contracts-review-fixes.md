---
format: aep.planning-md/3
id: story:contracts-review-fixes
kind: story
status: active
title: 'STT and TTS contracts: the six problems of the 0.3.0 review'
relations:
- decomposes: epic:contracts-and-testkit
- serves: vision:inference
- depends_on: story:tts-contract
- depends_on: story:audio-and-stt-contract
scope:
- confidence: cited
  path: crates/inference-stt
- confidence: cited
  path: crates/inference-testkit/src/stt.rs
- confidence: cited
  path: crates/inference-testkit/src/tts.rs
- confidence: cited
  path: crates/inference-testkit/tests/stt.rs
- confidence: cited
  path: crates/inference-testkit/tests/tts.rs
- confidence: cited
  path: crates/inference-text/src/lib.rs
- confidence: cited
  path: crates/inference-tts
revision: 10
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T08:33:20Z", actor: "human:timo", revision: 9}
- {from: "proposed", to: "active", at: "2026-10-19T08:33:20Z", actor: "human:timo", revision: 10}
---
## Context

A read-only review of the changes since 0.3.0 (`review-result:review-030-contracts`, 2026-10-19)
found six problems in the STT and TTS contracts and their mocks:

1. `OutputSegment` derives `Clone` (`crates/inference-text/src/lib.rs`), so a cloned segment can be
   offered to a second synthesis and played twice, against `crates/inference-tts/src/lib.rs:20-21`.
2. `RecognitionRequest::continuing` accepts any `i64` and the envelope sequence is incremented
   unchecked (`crates/inference-stt/src/lib.rs:489-492`): overflow at `i64::MAX`.
3. The STT and TTS mocks declare `max_concurrency` 1 and let two recognitions or syntheses run at
   once (`crates/inference-testkit/src/stt.rs:180-220`, `tts.rs:77-119`); design §23.2 requires
   declarations to match observed behaviour.
4. The mocks' docs say frames are taken as offered; they are taken on the next step
   (`stt.rs:11-13`, `tts.rs:12-13`).
5. Playback acknowledgement is documented as the sink's call only, but
   `SynthesisOutput::acknowledge_playback` is public to the adapter that owns the output
   (`inference-tts/src/lib.rs:470-478`).
6. The doc says no frame is emitted once a segment is Synthesized, while `complete` returns frames
   in the call that moves it to Synthesized (`inference-tts/src/lib.rs:44-49`, `:439-459`).

## Decisions

- `OutputSegment` is not `Clone`; code that cloned for comparison compares a snapshot.
- A continuing request refuses a first sequence below 1, and the sequence advance is checked: past
  `i64::MAX` the recognition refuses with a typed error.
- A mock with `max_concurrency` 1 refuses a second concurrent start with the typed concurrency
  refusal; a case asserts the declaration matches.
- Playback acknowledgement takes a value only a sink can make (a type the TTS contract exports for
  sinks, constructed from the delivered frames), so the synthesiser cannot confirm its own playback.
- The two doc mismatches are corrected to what the code does.

## Acceptance

`cargo test -p inference-stt -p inference-tts -p inference-text -p inference-testkit --all-features`
passes with one case per decision, each red before the fix.

## Out of scope

Voices, alignment, speculative TTS.
