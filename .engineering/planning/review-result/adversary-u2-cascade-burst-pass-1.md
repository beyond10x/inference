---
format: aep.planning-md/3
id: review-result:adversary-u2-cascade-burst-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on cascade turns in a burst: the turn a close completes gets no generation, a reopened turn drops its spans'
relations:
- reviews: story:cascade-turn-spans-in-a-burst
revision: 1
---
```
unit: story:cascade-turn-spans-in-a-burst, branch impl/cascade-turn-spans-in-a-burst at e0d575ee; cases committed as e53607a6
verdict: NEEDS-CHANGE
cases: executed 256→262, red 4
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 17 paths, all in the assigned adv1 scratch (part 6)
needs-coordinator: two calls. (1) Mutation ran inside the worktree as you allowed, which contradicts the adversary procedure; each file was restored with `git checkout`. (2) Should the reopened-turn case (finding 2) stay in this wave or go to backlog?
```

1. **`git --no-pager diff --stat e0d575ee..HEAD`** (the working tree is clean). Every path is a test file. The source files were changed only during the probe and mutant runs, and each was restored immediately (part 3).
```
 .../tests/adversary_u2_gate_turn_copies.rs         | 345 +++
 .../tests/adversary_u2_agent_reopened_turn.rs      | 257 +++
 .../tests/adversary_u2_stt_names_turn.rs           | 308 +++
 .../inference/tests/adversary_u2_turn_at_close.rs  | 122 +++
 4 files changed, 1032 insertions(+)
```

2. **Cases added.** Each red output below is from the first time the case ran on its own.
   - `crates/inference-pipeline/tests/adversary_u2_gate_turn_copies.rs`
     - `a_turn_completed_taken_in_the_drain_before_the_text_lanes_end_reaches_the_text_lane`: **red**.
       ```
       left: [Content(SpanCommitted {..}), Control(Stop)]
       right: [Content(SpanCommitted {..}), Content(Turn(Opened(..))), Content(Turn(Completed(..))), Control(Stop)]
       ```
     - `a_turn_completed_read_while_the_text_lane_ends_reaches_the_text_lane`: **red**.
       ```
       left: [SpanCommitted, Turn(Opened), Control(Stop)]
       right: [SpanCommitted, Turn(Opened), Turn(Completed), Control(Stop)]
       ```
     - `a_turn_completed_passed_on_both_outputs_is_recorded_as_passed_once`: **green**. It catches mutant M1: `left: 2`.
   - `crates/inference/tests/adversary_u2_turn_at_close.rs`, `the_turn_a_close_completes_is_answered_in_every_round`: **red**. Each round speaks 300 ms of tone then calls `close()` before the silence that would end the turn. 16 agents by 30 rounds.
     ```
     393 of 480 rounds went wrong, the first: Some("the model was asked [], not one generation over the turn the close completed")
     ```
   - `crates/inference-testkit/tests/adversary_u2_agent_reopened_turn.rs`, `a_span_held_when_a_turn_opens_over_an_uncompleted_one_is_answered_with_the_new_turn`: **red**.
     ```
     assertion `left == right` failed: no span held when the second turn opened is dropped
       left: ["first"]
      right: []
     ```
   - `crates/inference-testkit/tests/adversary_u2_stt_names_turn.rs`, `each_span_of_a_turn_names_the_turn_opened_the_node_sent_for_that_turn`: **green**. It catches mutant M2 deterministically (it panics at `:286`).

3. **Suite runs**, made after the cases existed. All exit 101, which is the expected result.
   - `cargo test -p inference-pipeline --all-features --no-fail-fast`: 153 passed, 2 failed (my two red gate cases).
   - `cargo test -p inference-testkit --features run,stt,llm,tts --no-fail-fast --test agent_turn_order --test agent_keeping_replies --test adversary_u2_agent_reopened_turn --test adversary_u2_stt_names_turn`: 5 passed, 1 failed (mine).
   - `cargo test -p inference --features testing --no-fail-fast --test cascade_burst --test recipes --test agent --test adversary_u2_turn_at_close`:
     - `cascade_burst` ok
     - `agent` 64 ok
     - `recipes` 35 ok
     - `adversary_u2_turn_at_close` FAILED: `428 of 480 rounds went wrong`
   - **How "before" was counted:** it is taken from these same runs with my 6 cases removed (3 pipeline, 2 testkit, 1 inference).
   - **Probe:** I temporarily added the missing copy inside `take_held` (`probe_copy_in_drain.diff`), then restored the file.
     - `adversary_u2_turn_at_close` passed 480 of 480.
     - The drain case turned green; the read-while-ending case stayed red.
     - So all of the failures at close come from this defect.
   - **Mutant M1:** drop `!copied` at `commit_gate.rs:541`. Only my third case catches it. No other inference-pipeline test reads `turns_passed` (checked with grep). I did not run the example's `adversary_rf1_first_audio`, which reads it.
   - **Mutant M2:** delete `stt.rs:401`. `cascade_burst` already caught it (140 of 960 rounds failed, 133 s), so it is not a surviving mutant; my STT case only makes the catch deterministic.

4. **Findings** (covering e0d575ee):

| # | file:line | what | verdict | origin |
|---|---|---|---|---|
| 1 | `crates/inference-pipeline/src/nodes/commit_gate.rs:611` (also `:596`) | Turn events the gate takes in `take_held` are passed only on the turn lane, never copied onto the text lane. `pass_copy` drops the copy once the text lane has ended in that drain. The agent therefore never sees the last turn's `TurnCompleted`: no generation starts, and the span is dropped silently (`dropped_inputs` is not exposed by the SDK). | NEEDS-CHANGE | introduced |
| 2 | `crates/inference-pipeline/src/nodes/agent.rs:532` (doc at `:42`) | A turn can open over one that never completed. The old turn's spans name the old `TurnOpened`, so `partition` never matches them and they are dropped at the end. The module doc says "a span held when a turn opens is the new turn's and is kept for it", and the base behaved that way. | INFEASIBLE | introduced |

   - **Finding 1, what was measured:** the gate cases above, plus 393 and 428 of 480 rounds red in `voice_cascade`.
   - **Finding 1, what reaches it:** `Agent::close` (or any drain) while a turn is open. The VAD and turn policy finish the turn at end of input, and the STT then sends `TurnCompleted`, the text-lane `stop` and the turns `stop` back to back (`stt.rs:600-606`). The same applies to an audio file that ends without trailing silence. The base had no copy path: the agent read turns through the turn split. The `local-voice-cascade` example uses the same gate, so it is reachable there too.
   - **Finding 1, fix to name (not applied):** copy turn events taken in `take_held` as well, and do not end lane 0 inside a drain before the copy is passed.
   - **Finding 2, what reaches it:** the turn policy opening a new turn after its store refused the previous completion (`turn_policy.rs:272-280`). The recipe's `Turns` store and the testkit `TurnStore` never refuse unless a caller moves the turn through `TurnStore::with`. In practice nothing found reaches it, hence INFEASIBLE.

5. **Attacked and could not break:**
   - `causation_id` gaining a new meaning: no non-test code reads it apart from the agent. The recogniser leaves it `None`, replay compares decisions, and the facade drops it.
   - Typed text mixed with speech: `Agent::commit` targets `user_input`, which `voice_cascade` does not expose.
   - Spans arriving after a turn's `TurnCompleted`: the STT emits only while a recognition runs.
   - Hypotheses and revisions: they carry the cause, but the gate holds them back.
   - The burst acceptance: 960 of 960 in the suite run.
   - The agent's own mutants (`is_none_or` changed to `is_some_and`, the cause ignored): `agent_turn_order` covers both.
   - The `graph.rs` edge-count wording: off by the exposures, same as at base.

6. **Paths written outside the worktree**, all in the unit's scratch directory `adv1/`: logs (`gate_red.log`, `close_red.log`, `testkit_cases.log`, `gate_cases2.log`, `stt_case.log`, `probe_gate.log`, `probe_close.log`, `m1_pipeline.log`, `m2_testkit.log`, `m2_burst.log`, `suite_pipeline.log`, `suite_testkit.log`, `suite_inference.log`), status files (`probe_status.txt`, `mutants_status.txt`), diffs (`probe_copy_in_drain.diff`, `m1.diff`, `m2.diff`), scripts (`probe_copy_in_drain.sh`, `mutants.sh`, `run_cases.sh`, `suites.sh`), other (`harness_head.rs`, `commit-msg.txt`). That is 24 files plus the `adv1/` directory, not the 17 the header says.

7. Findings block:
```findings
[
  {"file": "crates/inference-pipeline/src/nodes/commit_gate.rs", "line": 611, "category": "concurrency", "severity": "blocker", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "turn events the gate takes in its drain before a lane's end are never copied onto the text lane, and a copy after the text lane ended in that drain is dropped at :596, so the turn a close or drain completes gets no generation (393 and 428 of 480 rounds at 16 agents)"},
  {"file": "crates/inference-pipeline/src/nodes/agent.rs", "line": 532, "category": "contract-drift", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "a span naming the TurnOpened of a turn that never completed is never matched and is dropped at the end, contradicting the module doc at :42 that a span held when a turn opens is kept for the new turn; reachable only through a turn store that refuses a completion"}
]
```
