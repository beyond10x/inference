---
format: aep.planning-md/3
id: review-result:adversary-acknowledging-sink-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the acknowledging sink drain fixes
relations:
- reviews: story:acknowledging-sink-drain-ends
revision: 1
---
unit: 0.19.0 RF-audio-device story:acknowledging-sink-drain-ends, unit head f24f6f3; the adversary's cases are commit 1275edf (one test file)
verdict: NEEDS-CHANGE
cases: executed 17→20, red 2
origin: introduced 1 / pre-existing 0 / undecided 1

**Cases** (`adapters/inference-audio-device/tests/adversary_rf1_acknowledging.rs`, `--no-default-features --features pipeline`):
- red: `a_speaker_that_stops_while_a_frame_waits_for_room_fails_the_drain_within_the_patience`. With a frame pending for room, `exchange` does not poll input, so the end of input is never read and the playback watch never starts; a dead speaker hangs the drain. Reached by any answer longer than the 2 s queue, synthesized faster than real time.
- red: `a_heard_segment_whose_completion_never_comes_does_not_hold_back_the_next_answer`. A segment kept as heard at a turn's opening stays at the front of the order and blocks the next answer's receipts when its completion and supersede never come. The in-repository cascade always supersedes first.
- green: `a_heard_segment_kept_at_a_turn_is_no_clock_regression_for_the_next_answer_on_a_new_clock` (covers the `!held.heard` condition of the N31 check).

**Suite**: acknowledging 17 passed; adversary_rf1 1 passed, 2 failed; clippy and fmt clean.

**Residual** `superseded` forgetting a fully played, unacknowledged segment agrees with `SupersedeGeneration` in the stream domain; no fix belongs in this story.

Cargo 0.3 min, wall about 25 min, peak `target/` 152M.

```findings
[{"file":"adapters/inference-audio-device/src/acknowledging.rs","line":856,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"undecided","message":"while a frame waits for room in a full playout queue the sink never reads the end of input, so the playback watch never starts and a dead speaker hangs drain() forever instead of failing within the patience"},{"file":"adapters/inference-audio-device/src/acknowledging.rs","line":761,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"a segment kept as heard at a turn's opening whose completion and supersede never arrive blocks the receipts of the next answer; the in-repo cascade always supersedes the cut generation first, so only another producer reaches it"}]
```
