---
format: aep.planning-md/3
id: story:speech-node-speaks-whole-sentences
kind: story
status: draft
title: The speech path offers a synthesiser whole sentences
tags:
- from-finding
relations:
- informed_by: review-result:local-tts-adapter-adversary-1
- serves: vision:inference
revision: 1
---
## Context

The adversary pass on the local TTS adapter (review-result:local-tts-adapter-adversary-1, finding 3)
measured the local cascade with a real synthesiser bound: a 12-word sentence offered as 13 word
deltas gives 7133 ms of audio against 3186 ms offered as one segment. The speech node offers the
synthesiser one segment per committed text segment
(`crates/inference-pipeline/src/nodes/speech.rs:496-507`), and the cascade commits one segment per
delta of the language model. A hosted synthesiser joins the text of a context on its side; a local
one speaks each segment as an utterance of its own. The behaviour predates the adapter; the
adapter made it audible. `website/docs/status/limitations.md` states the limit.

## Planned scope

- Text reaches a synthesiser that speaks each segment on its own in units of at least a sentence:
  either the node before the speech node commits a segment per sentence, or the speech node joins
  committed deltas up to a sentence end before it offers them.
- A barge-in still discards text that was not spoken.

## Acceptance

- With the local synthesiser, a reply offered as word deltas lasts at most 1.2 times the same reply
  offered as one segment (state transition: the deltas of one sentence are committed).
- The first audio of a reply of two sentences starts before the second sentence is generated.
