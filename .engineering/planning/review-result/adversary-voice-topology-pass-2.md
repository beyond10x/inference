---
format: aep.planning-md/3
id: review-result:adversary-voice-topology-pass-2
kind: review-result
status: active
title: Adversary pass 2 on the voice topology latency report
relations:
- reviews: story:voice-topology-latency-per-step
revision: 1
---
unit: story:voice-topology-latency-per-step, tree inference-0-16-0-u4-topology, findings cover 28bab2b (the unit's head e214244 plus my test file)
verdict: NEEDS-CHANGE (1 warning: a panic on the arm's loop hangs `run_cascade` and `run_cascade_live`)
cases: executed 12→16, red 2 (12 = topology_latency 5, compare 5, adversary1 2, rerun after my cases with my file excluded; 16 adds adversary2 4)
origin: introduced 2 / pre-existing 0 / undecided 0 (main 33fd828 has no cascade arm)
wrote-outside-worktree: 6 paths, listed in part 6
needs-coordinator: no

1. `git diff --stat e214244 HEAD`: only `examples/realtime-voice/tests/adversary2_topology_workers.rs | 499 +` (a test file). Committed as 28bab2b through `b10x-gates bot`; author and committer are both `b10x-bot[bot]`.

2. Cases in `adversary2_topology_workers.rs`. Each runs under a 20 s wall-clock deadline. Command: `cargo-run.sh test -p realtime-voice --test adversary2_topology_workers`, first run 2 passed, 2 failed.
   - `a_playout_that_panics_fails_the_simulated_run_instead_of_hanging_it` (:296). **RED.** Red output: `panicked at …:239:9: adversary2: the playout failed`, then `…:58:13: the cascade run did not end within 20s of wall-clock time: it hangs`.
   - `a_synthesiser_that_blocks_on_the_callers_clock_does_not_hang_the_simulated_run` (:338). **RED** with the same deadline panic.
   - `a_synthesiser_that_panics_on_its_worker_fails_the_simulated_run`: green. A worker panic reaches the caller.
   - `the_live_arm_paces_the_barge_in_while_the_llm_and_the_synthesiser_block`: green. In `run_cascade_live`, the LLM waits 400 ms on the wall clock and the synthesiser blocks 8 ms per frame; barge-in to stop stays in [10, 80] ms with one stop and no notes.

3. Suite runs, after the cases existed:
   - `test -p realtime-voice --test topology_latency --test adversary1_topology_busy_step --test compare --test adversary2_topology_workers`: adversary1 2 passed; adversary2 2 passed, 2 failed. Cargo stops at the first red target (`rc=101 secs=23`).
   - `test -p realtime-voice --no-fail-fast --test topology_latency --test compare`: compare 5 passed, topology_latency 5 passed (`rc=0 secs=1`).
   - `clippy -p realtime-voice --all-targets --features local -- -D warnings`: **compiles clean** (`rc=0 secs=286`) after `task sherpa-onnx-archive`. The local recogniser handles are `Send` because sherpa-onnx's `OnlineStream` and `OnlineRecognizer` implement `Send` and `Sync`.
   - Cargo time 5.8 min (40 + 286 + 23 + 1 s). Wall about 25 min, most of it waiting for the build slot. Peak `target/` 2.2G. Cache discarded at the end.

4. Findings:
   - **`compare.rs:1084` with `:1256` (warning, NEEDS-CHANGE, introduced).** The generator worker's waker (`Prod`) holds a `Sender` to the worker's own task channel. That channel therefore never closes, and only `Generate::Stop` (sent in `cut()`) ends the worker.
     - If the loop thread panics while a generation runs (a caller's `Playout::play` or `stop`, its clock, or its `wait`), the runner is dropped without `cut()`. `thread::scope` then waits for the worker forever.
     - Before the workers, such a panic reached the caller; now it hangs. This applies to `run_cascade_live` and `compare` too.
     - Reached by: any caller-supplied `Playout`. Nothing in this repo panics today (`ClientPlayout` is a no-op).
     - Fix: a drop guard on the runner that sends `Stop`, or the worker holding a `Weak` sender.
   - **`compare.rs:1199` (note, INFEASIBLE, introduced).** A step that blocks on the caller's simulated clock deadlocks `run_cascade`: the clock moves only when no report is owed, and the blocked step owes one. This is how a fake of the ElevenLabs synthesiser, which blocks while it speaks, would behave.
     - Reached by: nothing in the repo; `compare` uses live mode.
     - Fix: a doc line saying a step must not block on the clock `run_cascade` is given.
   - **Judgement, not a red case (note).** A future that wakes itself and returns `Pending` until the fake clock passes some time also livelocks the simulated mode: each wake owes a report, so the clock never moves. `adversary1`'s `UntilPlayed` avoids this only through its `MAX_YIELDS` bound. Same doc fix as above.
   - **topology_latency hang question.** None of its 5 tests can hang with today's mocks: none blocks or wakes itself, and `DelayedPlayout` and `Fake` do not panic. They have no deadline, though. A regression in the owed-report counting (`Owing`), or a panic on the loop thread, would hang them while holding the cargo lock, as the first red case shows.

5. Attacked without a break:
   - Acceptance, both halves: per-step latencies for both topologies in one report, and the cascade skipped as "no hosted TTS configured".
   - Simulated pacing: no early clock advance, because `count` goes up before the clock-timer wake in `advance` returns and every report is sent before its count is released. Stale reports from a cut turn are dropped by `turn`.
   - `cut()` sends `Stop` before every join; queued tasks of a dead worker release their counts.
   - A worker panic propagates out of the run.
   - Live pacing holds while the LLM and the synthesiser block.
   - Feature `local` compiles.

6. Written outside the worktree: 6 scratch logs and a message file in the session scratch directory.

```findings
[
  {"file": "examples/realtime-voice/src/compare.rs", "line": 1084, "category": "concurrency", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "A panic on the cascade arm's loop (a caller's Playout, clock or wait) hangs run_cascade and run_cascade_live: the generator worker keeps a sender to its own task channel through its waker, so only Generate::Stop ends it and thread::scope never joins; red: adversary2_topology_workers.rs a_playout_that_panics_fails_the_simulated_run_instead_of_hanging_it."},
  {"file": "examples/realtime-voice/src/compare.rs", "line": 1199, "category": "concurrency", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "run_cascade deadlocks when a step blocks on the caller's simulated clock, as a fake of the blocking hosted synthesiser would; no caller in the repository does, and the doc does not state the limit; red: adversary2_topology_workers.rs a_synthesiser_that_blocks_on_the_callers_clock_does_not_hang_the_simulated_run."}
]
```
