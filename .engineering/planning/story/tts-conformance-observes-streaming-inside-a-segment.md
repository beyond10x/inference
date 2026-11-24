---
format: aep.planning-md/3
id: story:tts-conformance-observes-streaming-inside-a-segment
kind: story
status: draft
title: The TTS conformance check tells streaming inside a segment from delivery at its end
tags:
- from-finding
relations:
- informed_by: review-result:local-tts-adapter-adversary-1
- serves: vision:inference
revision: 1
---
## Context

The adversary pass on the local TTS adapter (review-result:local-tts-adapter-adversary-1, finding 2)
found that the conformance check `claims_observed`
(`crates/inference-testkit/src/tts/conformance.rs:671-727`) accepts the claim `output_streaming`
when frames arrive while the adapter's own record holds the segment Queued. Its default sentences
are single sentences. An adapter that delivers every frame of a segment at once, after the whole
segment is generated, passes the check: the local adapter delivered 366 frames of one sentence
within 0.2 ms of each other, 3.8 to 6.3 s after the offer. The contract page says the claim means
"emits partial results before completion" (`website/docs/concepts/contracts.md:42`).

## Planned scope

- `claims_observed` measures when frames arrive against when the segment's generation ends, or the
  contract page states the granularity the claim allows (a part per sentence).
- The check uses a text that can show the difference (more than one sentence).

## Acceptance

- A scripted synthesiser that delivers all frames of a two-sentence segment at once fails
  `claims_observed` for `output_streaming` (state transition: the segment completes).
- Every adapter that claims `output_streaming` today still passes, or its claim is corrected in the
  same change.
