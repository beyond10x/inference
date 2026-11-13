---
format: aep.planning-md/3
id: review-result:adversary-barge-in-flake-pass-1
kind: review-result
status: active
title: Adversary pass 1 on story:realtime-voice-barge-in-stop-flake
relations:
- reviews: story:realtime-voice-barge-in-stop-flake
revision: 1
---
unit: story:realtime-voice-barge-in-stop-flake, commit b35f7653 (impl/barge-in-stop-flake). Cases are on adv/barge-in-flake-1 at b13e34de.
verdict: NEEDS-CHANGE
cases: executed 94→98, red 2
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 14 paths, all in one scratch directory
needs-coordinator: whether F1 should block, given the story's scope line "without loosening what it proves"

**1. Diff stat** (b35f7653..b13e34de; only a test file)
```
 .../tests/adversary3_barge_in_flake.rs             | 500 +++++++++++++++++++++
 1 file changed, 500 insertions(+)
```

**2. Cases added** in `examples/realtime-voice/tests/adversary3_barge_in_flake.rs`

None of these cases changes the implementation. Each one holds the arm's loop at a chosen moment through the caller's `Playout::play`, which runs on the loop thread. The pace's grid comes from the time the recogniser first starts.

- `the_bound_rejects_a_cut_at_the_first_voiced_frame_on_a_loop_held_15ms`: **red**. This is the "cut at once, attack ignored" mutant, made with an attack of 1 ms. The loop is held 15 ms past the barge-in frame's due time. It was red 10 of 10 alone, measuring 15.1 to 25.1 ms. Output of the first run alone:
  ```
  a cut at the barge-in's first voiced frame, the attack of 30ms ignored, passes the case's bound of 10ms to 80ms: 15.098835ms from the due time (11.022µs from the send, 15.087813ms behind the pace); the loop was held from 10.118348ms before the slot to 15ms after it; stops 1
  test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3 filtered out; finished in 2.21s
  ```
- `the_bound_rejects_a_step_that_holds_the_loop_40ms_at_the_barge_in`: **red**, 10 of 10 alone, measuring 40.1 to 41.2 ms. First run alone:
  ```
  a loop held 40 ms at the barge-in sent both voiced frames in one burst and passes the case's bound of 10ms to 80ms: 40.092337ms from the due time (12.129µs from the send, 40.080208ms behind the pace); the loop was held from 14.617386ms before the slot to 40ms after it; stops 1
  ```
- `the_correct_runner_on_a_loop_held_15ms_stops_a_frame_after_the_due_time`: green. This is the control: with the correct attack and the same hold, the time from the due time is at least 20 ms (for example 4.77 ms from the send plus 15.38 ms of lag). It has a flaw of its own: its "the playout held the loop" precondition failed in 1 of 10 runs alone, and in the suite run below.
- `on_a_clock_that_overshoots_each_due_time_the_lag_is_the_overshoot`: green. On a simulated cascade clock whose wait overshoots each due time by 7 ms, `barge_in_sent_late` is exactly 7 ms and `barge_in_stop` exactly 20 ms.

**3. Suite run**, after the cases existed: `CARGO_INCREMENTAL=0 cargo +1.100.0 test -p realtime-voice --no-fail-fast` exited 101.
```
     Running tests/adversary3_barge_in_flake.rs
thread 'the_correct_runner_on_a_loop_held_15ms_stops_a_frame_after_the_due_time' panicked at examples/realtime-voice/tests/adversary3_barge_in_flake.rs:376:21:
precondition: the playout held the loop before the barge-in's slot
thread 'the_bound_rejects_a_step_that_holds_the_loop_40ms_at_the_barge_in' panicked at ...:448:5:
thread 'the_bound_rejects_a_cut_at_the_first_voiced_frame_on_a_loop_held_15ms' panicked at ...:422:5:
test result: FAILED. 1 passed; 3 failed; ...
error: 1 target failed:
```
- Every other binary passed: 95 passed, 3 failed, 1 ignored.
- The before count, 94, is this run minus my binary's 4 cases. I did not make a separate run.
- After this run I widened the control's hold window from 18 to 19.5 ms. I did not rerun the full suite after that change.

**4. Findings**

- **F1, `examples/realtime-voice/tests/adversary2_topology_workers.rs:523`, NEEDS-CHANGE, introduced.** The lower bound of 10 ms on stop plus lag accepts a cut at the first voiced frame whenever that frame went out 10 ms or more late.
  - The old bound measured from the send, saw about 0 ms, and rejected this mutant on any host.
  - What reaches it: any change in the runner that ignores the attack, combined with ordinary host lag. In 120 runs of the unit's own case, the lag was 10 ms or more in 11 of them (2/40 quiet, 7/40 with 20 CPU burners, 2/40 with 40). The mutant would slip through in those runs.
  - Fix, named and not applied: set the lower bound to `FRAME` (20 ms). The correct runner cannot stop before the second voiced frame is due. In those 120 runs the minimum was 20.0 ms, and the implementor's 50 runs gave 20.0 to 41.7 ms. With that bound the mutant only slips at a lag of 20 ms or more, which was 5 of the 120 runs.
- **F2, same line, INFEASIBLE, introduced.** A step that holds the loop for roughly 20 to 60 ms at the barge-in now passes as host lag. The old bound rejected it, but only in the same way it rejected real host lag, which was the flake. From timestamps alone the two cannot be told apart.
- **F3, `examples/realtime-voice/src/compare.rs:764`, CONFIRMED, introduced.** The native arm sets `barge_in_sent_late`, but no case reads it. If that line is deleted the suite stays green. Nothing outputs the field except `Debug`: it is not in `runs_json` or the table.

**5. Attacked and could not break**
- **Upper bound:** stop plus lag is never less than stop, so every run the old upper bound failed, the new one also fails. A late cut or a long held step stays red.
- **80 ms upper bound under load:** 0 of 120 runs went red. The maximum was 38.1 ms quiet, 44.8 ms with 20 burners and 62.0 ms with 40.
- **Due time:** on the simulated clock with a 7 ms overshoot the lag measured exactly 7 ms. The native arm passes its due time to `send` the same way (`compare.rs:644`); I read it but did not run it.
- **Frames sent without pacing, or a cut that never happens:** I reasoned these through but did not run them. Frames sent ahead of the grid give a lag of 0 and a total of about 0, so the case goes red. A missing cut fails `stops == 1` or the `expect`.
- **8 kHz synthesiser:** reasoned, not run. The loop still gets one report every 8 ms. The change removes the path where the playout runs dry. It also makes visible a mutant that ignores the cut and measures to the end of the audio: at 8 kHz that is about 110 to 140 ms past the due time, against 15 to 20 ms at 16 kHz.

**6. Paths written outside the worktree**, all in `<scratch>/p2/wave2/flake-adv/`:
`build.log`, `commit-msg.txt`, `control.log`, `hold40.log`, `load.log`, `loop.sh`, `mutant.log`, `sherpa.log`, `stats.py`, `suite.log`, `suite2.log`, `unit-load20.log`, `unit-load40.log`, `unit-quiet.log`.

Two things the brief asked for go against the adversary procedure's general rules: I switched branches and deleted `target/`. The worktree is back on impl/barge-in-stop-flake at b35f7653 and clean. My lease is released and no CPU load is left running.

```findings
- file: examples/realtime-voice/tests/adversary2_topology_workers.rs
  line: 523
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a lower bound of 10 ms on stop plus lag accepts a cut at the first voiced frame whenever that frame went out 10 ms or more late (11 of 120 measured runs), where the old bound always rejected it; a bound of FRAME separates the two
- file: examples/realtime-voice/tests/adversary2_topology_workers.rs
  line: 523
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a step holding the arm's loop for roughly 20 to 60 ms at the barge-in now passes as host lag, and timestamps alone cannot tell the two apart
- file: examples/realtime-voice/src/compare.rs
  line: 764
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the native arm's barge_in_sent_late is read by no case and output nowhere, so deleting the assignment leaves the suite green
```