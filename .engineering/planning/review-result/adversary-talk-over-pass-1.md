---
format: aep.planning-md/3
id: review-result:adversary-talk-over-pass-1
kind: review-result
status: active
title: 'Adversary pass on the talk-over cases: two pre-existing defects and a mutant the window let through'
summary: Two red cases, both pre-existing, fixed by the one correction; the cascade hang explained and fixed in the sink
relations:
- reviews: story:long-talk-over-does-not-resume-the-cut-answer
revision: 1
---
## Pass

This is the adversary pass on the talk-over cases at `e5e5b9b`. Its cases are in `e694898`, test
files only: `adversary1_talk_over_cut_answer.rs` and `adversary1_talk_over_sink_order.rs`.

verdict: NEEDS-CHANGE
cases: executed 64→70, red 2
origin: introduced 3 / pre-existing 2 / undecided 0

## Cases

### Red, pre-existing at `232808a`

Both defects were already in the code before this story.

- **`Conversation` replays audio it truncated**
  (`audio_of_the_response_conversation_truncated_at_the_barge_in_does_not_play_after_it`).
  - A session with `auto_interrupt` on that supports truncation records its in-flight deltas
    after the barge-in, as the Realtime binding does.
  - `Conversation` truncates that response itself, then queues and plays those deltas.
- **The cascade sink loses the interruption record**
  (`a_supersede_taken_before_the_talk_overs_opening_still_records_its_interruption`).
  - `GenerationSuperseded` and `TurnOpened` reach the acknowledging sink on two edges, and
    nothing orders those edges.
  - When the supersede is taken first, the opening parks the interruption for a supersede that
    has already come, and the record never arrives.
  - This explains the cascade 6 s case's one wait of 60 s in eight runs. It was not reproduced
    in 20 runs.

### A mutant the unit's cases let through

The unit's cases read only up to the next answer's first tick. A session that resumes the cut
answer behind the next answer passed them.

### Real bindings against the scenario the unit pinned

The scenario: a provider keeps streaming the cut answer after speech started.

- **GPT-Live:** reachable. The response stays current after the speech is heard, so later
  audio arrives on the same response. Nothing on the event tells the rest of the cut sentence
  from the model's next words.
- **Realtime with `auto_interrupt` on:** reachable. No shipped configuration sets that.
- **Nova Sonic:** not reachable. Its interruption notice records the speech and cancels the
  response in one call, and the ledger refuses audio for a response that is not in progress.

## Routing

The unit's one correction made these fixes:

- `2a2410d`: the sink records the interruption in either order of its edges. The cascade
  `talk_over` case then passed 10 sequential runs.
- `8eecdb8`: `Conversation` drops later audio of a response it truncated. GPT-Live declares no
  truncation, so its next words on the same response still play.
- `28103b7`:
  - the cases read until the speaker has been idle for 25 ticks;
  - the unreachable Nova pin is removed;
  - a Realtime case with `auto_interrupt` on is added;
  - the GPT-Live pin stays for story:cut-response-audio-is-told-from-the-next-words.
- `33bc0ea`: the adversary's resume mutant re-speaks the cut answer under a fresh response,
  because the truncation fix now drops a resume under the truncated one.
  - A throwaway copy of the read-to-the-end case failed against both resume mutants, with "the
    cut answer plays again after the talk-over started", and passes without them.

```findings
[{"file":"examples/realtime-voice/src/lib.rs","line":437,"category":"acceptance","severity":"warning","verdict":"INFEASIBLE","origin":"pre-existing","message":"with auto_interrupt on and a truncating session, Conversation plays audio of the response it truncated at the barge-in"},{"file":"adapters/inference-audio-device/src/acknowledging.rs","line":418,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"a GenerationSuperseded taken before the turn's TurnOpened stops the speaker, then turn_opened parks the interruption for a supersede already taken, so the talk-over's interruption is never recorded"},{"file":"examples/local-voice-cascade/tests/talk_over.rs","line":381,"category":"concurrency","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the case waits 60 s for an interruption record that only one of two unordered edge deliveries produces"},{"file":"examples/realtime-voice/tests/talk_over.rs","line":471,"category":"mutant","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"the cases read ticks only up to the next answer's first tick, so a session resuming the cut answer behind the next answer passes"},{"file":"examples/realtime-voice/tests/talk_over.rs","line":714,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"the Nova streaming pin reaches a state the Nova binding cannot produce, only through the test wrapper"}]
```
