---
format: aep.planning-md/3
id: story:segment-linear-transitions
kind: story
status: implemented
title: 'Output segments: transitions that consume, and the LLM mock''s concurrency'
relations:
- decomposes: epic:contracts-and-testkit
- serves: vision:inference
- depends_on: story:contracts-review-fixes
scope:
- confidence: cited
  path: crates/inference-llm
- confidence: cited
  path: crates/inference-testkit/src/llm.rs
- confidence: cited
  path: crates/inference-testkit/src/stream.rs
- confidence: cited
  path: crates/inference-testkit/tests
- confidence: cited
  path: crates/inference-text/src/lib.rs
- confidence: cited
  path: crates/inference-tts
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T10:59:23Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T10:59:23Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-19T11:39:31Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":4,"verification":1}}}
---
## Context

Wave 10 (`story:contracts-review-fixes`) made `OutputSegment` not `Clone`, which closes the explicit
clone route to offering one segment to two syntheses. Its implementor found two routes left open:

1. The transitions on `OutputSegment` take `&self` (`crates/inference-text/src/lib.rs`), so calling
   `tentative.commit()` twice yields two Committed values with the same identity, and two syntheses
   each accept one (`crates/inference-tts/tests/synthesis_output.rs` builds that pair). The
   inference-tts crate doc names the gap.
2. The LLM mock declares `max_concurrency` 1 and lets a second generation start
   (`crates/inference-testkit/src/llm.rs`), the mismatch the STT and TTS mocks had; design §23.2
   requires declarations to match observed behaviour.

## Decisions

- `OutputSegment` transitions consume the segment (`commit(self)`, and likewise the others), so one
  identity yields at most one Committed value. Callers that inspect a segment before and after a
  transition compare `SegmentSnapshot` values.
- The LLM mock with `max_concurrency` 1 refuses a second concurrent start with the typed
  concurrency refusal, as the STT and TTS mocks do since wave 10.

## Acceptance

`cargo test -p inference-text -p inference-tts -p inference-llm -p inference-testkit --all-features`
passes with: a compile_fail doctest that commits one tentative segment twice; a case offering the
same committed identity to two syntheses is no longer constructible; a case in which the LLM mock
refuses a second concurrent generation. Each red before the fix.

## Out of scope

Voices, alignment, speculative TTS.

## Identity minting

Decided in wave 12:

- `OutputSegment::emit_as` keeps accepting any `SegmentId`: the testkit's identity counter needs
  it. Two values of one identity can still be made through it; its docs and the inference-tts crate
  doc name that route. Closing it would take a non-`Copy` identity or a sealed allocator, left for
  a story that needs it.
- A refused transition hands the segment back unmoved inside `SegmentMoveRefused`.
