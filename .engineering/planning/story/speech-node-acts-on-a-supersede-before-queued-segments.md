---
format: aep.planning-md/3
id: story:speech-node-acts-on-a-supersede-before-queued-segments
kind: story
status: draft
title: The speech node acts on a supersede before queued segments are generated
tags:
- from-finding
relations:
- informed_by: review-result:local-tts-adapter-adversary-1
- serves: vision:inference
revision: 1
---
## Context

The adversary pass on the local TTS adapter (review-result:local-tts-adapter-adversary-1, finding 4)
asked what a barge-in costs while a local synthesiser generates. The correction measured it with a
scripted recogniser and generator and the real synthesiser, three committed segments, superseded
while the first is generated: the last frame of the superseded generation left the synthesiser
2275 ms after the supersede in one run and 5671 ms in a second. The speaker stopped at once and all
399 frames of the superseded answer were dropped as stale, so nothing stale was heard; the cost is
that the first frame of the next answer came 760 ms and 1072 ms after that last frame. The drive
thread was not held (a 10 ms ticker's largest gap was 10.7 ms and 12.0 ms).

Cause: after it offers a committed segment the speech node calls `pump`
(`crates/inference-pipeline/src/nodes/speech.rs:507`), which loops the blocking iterator
`self.synthesis.next()` (`speech.rs:422-448`). The node reads its text port again only when that
segment is generated, so every segment committed before the supersede is generated in full and the
`GenerationSuperseded` is acted on after them. A hosted synthesiser answers faster than real time,
so the wait was short there; a local one generates at about half real time. The behaviour predates
the adapter. `website/docs/status/limitations.md` states the limit with both runs.

## Planned scope

- The speech node reads its synthesis without blocking its text port: `pump` and `step`
  (`speech.rs:334-352`) take synthesis events with `poll_event` inside the node's select, so
  `interrupted` discards queued segments before they are generated.
- A segment being generated at the supersede is cancelled where the synthesiser can cancel it.

## Acceptance

- With a synthesiser that takes 1 s per segment and three committed segments, a supersede during the
  first segment is followed by the first frame of the next answer within 1.5 s (state transition:
  the generation is superseded).
- No frame of the superseded generation reaches the speaker after the supersede.
