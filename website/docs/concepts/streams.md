---
title: Streams and commit rules
sidebar_label: Streams
sidebar_position: 3
description: What is provisional, what is committed, what the listener actually heard, and what a barge-in does to each.
lede: Speech systems change their minds, so a pipeline keeps apart what may still change, what is final and what was played.
source: crates/inference-text, crates/inference-stt, crates/inference-tts, crates/inference-tools, ess/domains/stream.yaml, examples/realtime-voice, examples/local-voice-cascade
---

# Streams and commit rules

Speech systems change their minds. A pipeline keeps apart what may still change, what is final and
what the listener actually heard.

## Inbound: provisional and committed

```text
Hypothesis ──revise──▶ Hypothesis ──commit──▶ Committed (immutable)
```

- A **hypothesis** span may be replaced by a revision.
- A **committed** span is immutable: a revision of it is refused (`CommittedSpanImmutable`).
- **Turn completion** is a separate control boundary, not the commit of a span.

## Outbound: tentative and played

```text
Tentative ─commit─▶ Committed ─queue─▶ Queued ─synthesize─▶ Synthesized
                                                                 │
                                         sink acknowledges ──────┴──▶ Played

any state before Played ─discard─▶ Discarded
```

- An LLM's text is **tentative** until committed. Synthesis consumes committed segments only: a
  tentative one is refused.
- Only the media sink moves a segment to **played**, with a receipt it makes from the frames it
  actually played. The synthesiser cannot.
- A played segment cannot be discarded: speech that was heard cannot be unsaid.
- Each transition consumes the segment, so a segment commits once and is queued once.

## Default commit rules

- Side-effecting tools need committed user input. A call on a provisional span is refused; the
  explicit speculative path admits read-only tools only.
- TTS consumes committed output segments, not tentative tokens.
- Cancellation suppresses stale unplayed media and uncommitted output from older generations.

## Barge-in

Each generation is an epoch of its node. When the user starts speaking over an answer:

1. the current generation is superseded;
2. the speaker stops and its unplayed queue is cleared;
3. unplayed segments of that generation are discarded, and stale frames are refused at the sink;
4. the new input is admitted.

Cancelling delivery and stopping computation are distinct: a backend may keep computing after
cancel, and an adapter claims `interruptible` only when it can stop work.

| Where it runs | Since |
|---|---|
| the native speech example: the speaker stops and the played position truncates the model's answer | 0.5.0 |
| tool calls under interruption | 0.5.0 |
| the local voice cascade: commit gates, supersede, discard, stale frames refused at the sink | 0.6.0 |
