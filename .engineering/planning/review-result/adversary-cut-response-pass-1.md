---
format: aep.planning-md/3
id: review-result:adversary-cut-response-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the GPT-Live cut-response unit
relations:
- reviews: story:cut-response-audio-is-told-from-the-next-words
revision: 1
---
unit: 0.17.0 U1 story:cut-response-audio-is-told-from-the-next-words, unit head 9fee977; the adversary's cases are commit d904dde (one test file)
verdict: NEEDS-CHANGE
cases: executed 60→62, red 3
origin: introduced 2 / pre-existing 0 / undecided 0

**Cases** (`adapters/inference-openai/tests/adversary2_u1_live_boundary.rs`, scripted local server; the caller speaks in two input fragments and the cut sentence continues after the first fragment ends):

- `the_cut_sentence_that_outruns_the_first_input_fragment_stays_on_the_cut_response`: red. Got `[" Once upon a time there was a ship", " today Sure."]`, expected `[" Once upon a time there was a ship today", " Sure."]`. The new response opens on " today", still inside the caller's speech.
- `no_frame_of_the_cut_sentence_is_on_a_response_not_spoken_over_during_the_talk_over`: red. Three responses; " today" on its own, its voiced frame before the second speech start, so it would play during the talk-over.

**Suite** on 13 GPT-Live targets of `inference-openai`: all green except `adversary1_gpt_live_speech_over_timing` 4/1 (`a_second_interruption_of_the_same_response_is_reported`: a resumption after the speech ended is now a new response, so the replay stops before the second interruption) and the two new cases. realtime-voice: acceptance 3, adversary1_gpt_live_transcript 6, adversary1_talk_over_cut_answer 5, adversary2_nova_barge_in 1, talk_over 22, wiring 12, all green.

**Attacked, not broken**: no duplex type or trait changed, no counted drops; per-response usage, usage after the boundary moves to the new response; Realtime with `auto_interrupt` on and an empty speaker drops in-flight deltas; a fragment without `start_ms` is counted; Nova only marks responses its ledger ended.

**Reachability**: the unit's own replay already has an output fragment running past the first input fragment's end; how the hosted service's input transcripts lag was not observed.

Cargo 1.6 min, wall 22 min, peak `target/` 1.8G.

```findings
[
{"file":"adapters/inference-openai/src/live_duplex.rs","line":749,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The boundary compares against the speech end seen so far, so a cut-sentence fragment starting after the first input fragment but inside the caller's continuing speech opens the playable response and the rest of the cut answer plays."},
{"file":"adapters/inference-openai/tests/adversary1_gpt_live_speech_over_timing.rs","line":240,"category":"contract-drift","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"a_second_interruption_of_the_same_response_is_reported is red at 9fee977 because a resumption after speech now completes the response; the unit neither ran nor updated this existing target."}
]
```
