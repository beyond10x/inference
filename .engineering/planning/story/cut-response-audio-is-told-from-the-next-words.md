---
format: aep.planning-md/3
id: story:cut-response-audio-is-told-from-the-next-words
kind: story
status: active
title: Audio of a cut response is told from the next words
summary: Mark duplex output audio so the cut part of a response is dropped and the model's next words on it still play
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
- depends_on: story:long-talk-over-does-not-resume-the-cut-answer
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: cited
  path: adapters/inference-openai/src/live_duplex.rs
- confidence: inferred
  path: adapters/inference-openai/tests/gpt_live_duplex.rs
- confidence: inferred
  path: crates/inference-duplex/src/contract.rs
- confidence: inferred
  path: crates/inference-duplex/src/ledger.rs
- confidence: inferred
  path: ess/domains/duplex.yaml
- confidence: cited
  path: examples/realtime-voice/src/lib.rs
- confidence: cited
  path: examples/realtime-voice/tests/adversary1_gpt_live_transcript.rs
- confidence: cited
  path: examples/realtime-voice/tests/talk_over.rs
- confidence: inferred
  path: website/docs/guides/talk-to-a-hosted-voice-model.md
revision: 15
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T14:22:42Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-05T16:11:00Z", actor: "human:operator", revision: 15}
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

## Scope after the talk-over unit

Narrowed by the coordinator at the talk-over unit's close, after its adversary pass, which read
the bindings:

- **GPT-Live:** in scope. After speech is heard, its response stays current, so later audio
  arrives on the same response. Nothing on the event tells the rest of the cut sentence from the
  model's next words.
- **Nova Sonic:** out of scope. It cannot reach the state: its interruption notice records the
  speech and cancels the response in one call, and the ledger refuses audio for a response that
  is not in progress.
- **Realtime with `auto_interrupt` on, truncated by `Conversation`:** fixed in the talk-over unit.
  `Conversation` drops later audio of a response it truncated.
- **Added to scope:** with `auto_interrupt` on, the speaker can already be empty when speech
  starts. Then nothing is truncated, and audio deltas still in flight for that response would
  play. This is the same class as GPT-Live.

The acceptance reads accordingly:

- The pinned GPT-Live tests flip.
- A case with `auto_interrupt` on and an empty speaker at the speech start plays no in-flight
  audio of the cut response.
- `adversary1_gpt_live_transcript.rs` stays green.

## Scope

Derived 2026-11-05 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `examples/realtime-voice/src/lib.rs`, `Conversation::handle` (the `SessionEvent::OutputAudio` arm, l. 441-446) and `Conversation::barge_in` (l. 521-565): it must drop the cut part and play the next words, and drop in-flight audio when the speaker is empty at the speech start (`barge_in` returns early at l. 532-534 and nothing is truncated) — cited
- **Files (binding form):** `adapters/inference-openai/src/live_duplex.rs`: `speech_over` (l. 703-723) and `Spoken.interrupted` (l. 449-455, 938-947) know the interruption interval, and `output` (l. 725-775) is where a frame gets its response — cited
- **Fact for the form choice:** GPT-Live's `session.output_audio.delta` carries only the audio (`LiveEvent::OutputAudio { audio }`, `adapters/inference-openai/src/live.rs:354`, `:438`). There is no item index, content index or turn mark, so a mark must come from transcript timing (`OutputTranscript`/`InputTranscript` `start_ms`/`end_ms`, live.rs:355-365) or from the binding opening a new response for the next words — cited
- **Fact for the form choice:** `Conversation<S: SpeechSessionHandle>` sees only `SessionEvent` and `ledger()` (lib.rs:264, 292). A mark kept inside the binding's own types never reaches it, so the binding form means the binding opening a new `ResponseId` at the next words through existing events — inferred
- **Files (tests):** `examples/realtime-voice/tests/talk_over.rs` (`gpt_live_streaming_on`, l. 753-787: the three `..._play_today_...` tests flip; `MarkingSession` l. 252) and `examples/realtime-voice/tests/adversary1_gpt_live_transcript.rs`, whose `LiveSession` mock (l. 115-188) mirrors `live_duplex.rs`; `repeated_fragments_stop_the_answer_once_and_later_audio_plays` (l. 386-413) must stay green — cited
- **Files (contract form, instead):** `crates/inference-duplex/src/lib.rs:412-418` (`OutputAudio`), `crates/inference-duplex/src/ledger.rs:1194-1209`, `crates/inference-duplex/src/contract.rs:313-361`, `ess/domains/duplex.yaml:766-804, 1256` (spec first), `crates/inference-testkit/tests/target/duplex.rs:321, 1008` — cited
- **Compile-unchanged constraint:** `OutputAudio` has public fields and no `#[non_exhaustive]`; a new field breaks struct literals at `crates/inference-testkit/src/duplex.rs:364`, `examples/realtime-voice/tests/talk_over.rs:236`, `examples/realtime-voice/tests/adversary1_talk_over_cut_answer.rs:467`. A new `ledger.output_audio` parameter breaks three binding call sites; a new `SessionEvent` variant breaks exhaustive matches. Only a new ledger method or a provided default method on `SpeechSessionHandle` keeps all 15 implementers unchanged — cited (sites), inferred (which additions stay unbroken)
- **Fewer files:** the binding form, about 4 files; the contract form adds about 3-5 more — inferred
- **Also likely:** `adapters/inference-openai/tests/gpt_live_duplex.rs`; a Realtime case with `auto_interrupt` on and an empty speaker in `talk_over.rs` (`openai_interrupting_streaming_on`, l. 730) — inferred
- **Documents:** module docs of `live_duplex.rs` (l. 24-35), `lib.rs` (l. 9-22) and `talk_over.rs` (l. 29-35); `website/docs/guides/talk-to-a-hosted-voice-model.md`, `CHANGELOG.md` `[Unreleased]` — inferred
- **Not in scope:** Nova Sonic (`adapters/inference-aws`) — cited
- **Confidence:** high for `Conversation` and the tests; medium for the binding-versus-contract split
- **Would collide with:** any unit touching `examples/realtime-voice/src/lib.rs` or `tests/talk_over.rs`, the GPT-Live binding, and with the contract form the `inference-duplex` event and ledger surface and `ess/domains/duplex.yaml`. The 0.16.0 topology unit does not touch `lib.rs`
- **Not established:** whether output transcript `start_ms` is a reliable boundary for the next words (the timeline is read from protocol documents, not observed); whether the Realtime binding records the cancel before or after in-flight deltas
