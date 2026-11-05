---
format: aep.planning-md/3
id: story:cut-response-audio-is-told-from-the-next-words
kind: story
status: proposed
title: Audio of a cut response is told from the next words
summary: Mark duplex output audio so the cut part of a response is dropped and the model's next words on it still play
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
- depends_on: story:long-talk-over-does-not-resume-the-cut-answer
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T14:22:42Z", actor: "human:timo", revision: 2}
---
## Context

The talk-over cases of story:long-talk-over-does-not-resume-the-cut-answer are green: after a
talk-over of 0.5, 6 or 12 s, no frame of the cut answer plays once the talk-over ends.

One scenario is not held. A session runs with `auto_interrupt` on (GPT-Live, Nova Sonic). The
provider keeps streaming audio of the response after speech started. `Conversation` in
`examples/realtime-voice` then plays those frames during the talk-over.

The obvious fix fails. Dropping all later audio of a response that a barge-in stopped breaks
`adversary1_gpt_live_transcript.rs`. GPT-Live speaks its next words on the same response after a
stop, and those words must play. On one response, `Conversation` cannot tell the rest of the cut
sentence from the model's next words.

Today's behaviour is pinned in `examples/realtime-voice/tests/talk_over.rs` by the tests named
`frames_of_the_cut_answer_after_the_speaker_ran_dry_play_today_during_the_talk_over`.

## Planned scope

Mark output audio so that a frame of the cut part of a response can be told from audio the model
produced after the barge-in. The change is additive. Two forms are possible:

- in the binding, with the provider's item or content index, or its turn mark, carried on the
  frame's event;
- an additive extension of the `inference-duplex` contract, such as an optional mark on the audio
  event.

There is no breaking contract change.

`Conversation` then drops the cut part and plays the next words.

## Acceptance

- The pinned `..._play_today_during_the_talk_over` tests flip. No frame of the cut part plays
  during or after the talk-over, on GPT-Live and Nova Sonic.
- `adversary1_gpt_live_transcript.rs` stays green: audio the model speaks after the stop on the
  same response plays.
- Every existing `inference-duplex` implementer compiles unchanged.
