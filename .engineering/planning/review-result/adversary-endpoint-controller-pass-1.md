---
format: aep.planning-md/3
id: review-result:adversary-endpoint-controller-pass-1
kind: review-result
status: active
title: Adversary, story:endpoint-controller, pass 1
relations:
- reviews: story:endpoint-controller
revision: 1
---
unit: story:endpoint-controller, working tree at base cf6bc47 plus uncommitted changes (impl-endpoint-controller tree)
verdict: CONFIRMED (1 red case: a drained run leaves its turn Open)
cases: executed 90→95, red 1
origin: introduced 7 / pre-existing 0 / undecided 0
wrote-outside-worktree: 8 paths, all in the assigned scratch adversary-1/ (part 6)
needs-coordinator: none

Coordinator follow-up: all three steps were already done before the interruption. The mutant copy and its `target/` (324M) were deleted, and the scratch listing confirms it is gone. The gate ran after my cases existed; the tree has not changed since, so I did not run it again. My lease was ended cleanly (`session-end`, exit 0) after the gate, so there was no expired lease to re-acquire. Disk is at 14G free.

**1. `git --no-pager diff --stat`**
```
 Cargo.lock                                 |  2 ++
 crates/inference-pipeline/Cargo.toml       |  2 ++
 crates/inference-pipeline/src/nodes/mod.rs | 10 +++++++++-
 crates/inference-runtime/Cargo.toml        |  2 +-
 crates/inference-testkit/src/lib.rs        |  3 ++-
 crates/inference-testkit/src/stt.rs        |  9 +++++++++
```
These are all the implementor's changes. Mine is one new untracked test file, `crates/inference-runtime/tests/adversary1_endpoint_controller.rs` (459 lines, no host path in it). I did not touch any implementation file.

**2. Cases added** (all in `crates/inference-runtime/tests/adversary1_endpoint_controller.rs`, run through the real `RunController::drive`, same pipeline as the unit's test)

| Case | Asserts | Now |
|---|---|---|
| `a_drained_run_leaves_no_turn_open` | release the first tone, wait until the turn is Open, call `RunRequests::drain()`; the run ends Completed and the store's turn is Completed | **RED** |
| `the_end_of_input_during_speech_completes_the_open_turn` | 300 ms of tone then end of input: TurnOpened@480, TurnCompleted@4800, turn Completed | green, kills M1 |
| `a_level_exactly_at_the_threshold_is_loud` | constant 16384/32768 against threshold 0.5 opens a turn (@480, completes @1600) | green, kills M2 |
| `an_attack_time_between_samples_rounds_up` | attack 30.031 ms at 16 kHz (480.5 samples) opens on the 4th frame (@640) | green, kills M3 |
| `every_turn_event_names_its_cause` | all 4 turn events have a `causation_id`, and all 4 differ | green, kills M4 |

Red output from the first run of the file on its own (`cargo test -p inference-runtime --test adversary1_endpoint_controller`, exit 101):
```
thread 'a_drained_run_leaves_no_turn_open' panicked at crates/inference-runtime/tests/adversary1_endpoint_controller.rs:384:5:
assertion `left == right` failed: the drained run left its turn Open; the sink received [("inference.stream.TurnOpened", AudioFrameTime, 480)]
  left: [Open]
 right: [Completed]
test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

Mutants: one copy of the tree in scratch, mutants applied one at a time, each run against the unit's `endpoint_controller` tests (2), the pipeline `nodes::` unit tests (8) and my file.

| Mutant | Unit suite | Mine |
|---|---|---|
| M1 `vad.rs:295-298` removed (speech not ended at `stop`) | 2/2 + 8/8 green | end-of-input and threshold cases red |
| M2 `vad.rs:156` `>=` changed to `>` | green | threshold case red |
| M3 `vad.rs:95` `div_ceil` changed to `/` | green | rounding case red |
| M4 `turn_policy.rs:265` removed (no `causation_id`) | green | cause case red |

**3. Gate** (run after the cases existed)
- `cargo test -p inference-runtime --all-features --no-fail-fast`: **exit 101**. 95 cases ran, 94 passed and 1 failed (`a_drained_run_leaves_no_turn_open`); every other binary reported `test result: ok`, including `endpoint_controller` 2/2. The "before" count of 90 is this same run minus my 5 cases.
- `cargo clippy -p inference-runtime --all-targets --all-features -- -D warnings`: exit 0
- `cargo fmt -p inference-runtime --check`: exit 0

**4. Findings**
- **F1 drain leaves the turn Open** (warning, CONFIRMED, introduced)
  - What was measured: the assertion at `adversary1_endpoint_controller.rs:384`, exit 101.
  - What reaches it: the public `RunRequests::drain()` (`drive.rs:227`, DrainRun).
    - On drain, the testkit source closes its edge without sending `stop` (`graph/nodes.rs:783`).
    - The detector treats a closed edge as "ended" and does not end speech (`vad.rs:292`).
    - The turn policy also returns on a closed edge with its turn still open (`turn_policy.rs:277`).
  - Result: a Completed run keeps an Open turn that the single turn authority can no longer complete.
  - Fix to name, not applied: handle a closed edge like `stop`. The detector sends `Ended` before returning, and the policy completes its open turn on a closed edge or `stop`.
- **F2 store refusal of `OpenTurn` fails the node** (note, NEEDS-CHANGE, introduced)
  - `turn_policy.rs:238`: when the store refuses `OpenTurn`, the node fails.
  - `ess` declares `run-not-active` for a Draining run (`stream.yaml:405-427`), and the testkit already implements it (`open_turn_in`, `stream.rs:1299`). So a speech onset decided from frames still buffered during a drain would fail the drain.
  - What reaches it: nothing found. No store in the tree connects `open_turn_in` to the turn policy, and the unit's test uses the `open_turn` that never refuses.
- **F3 activity kinds not declared in ess** (note, NEEDS-CHANGE, introduced)
  - `speech_started` / `speech_ended` (`vad.rs:60-62`) are not declared in `ess/`. `ess` says an envelope's kind is the declared name of its event (`stream.yaml` EventEnvelope).
  - The implementor disclosed this.
- **F4 doc drift on the end-of-speech offset** (note, NEEDS-CHANGE, introduced)
  - `vad.rs:20-21` and `:53-54` say speech still going at end of input ends "just past the last frame".
  - `Detector::end` (`vad.rs:193`) instead returns the start of a quiet run in progress, and the unit test at `vad.rs:476` asserts 80, not 90.
- **F5 four surviving mutants** (M1-M4, note, CONFIRMED, introduced): the unit's suite does not test these lines. The cases in part 2 now cover them.

**5. Attacked, could not break**
- The `stt-only` example's dependency tree still excludes `inference-runtime` (`cargo tree -p stt-only -e normal -i inference-runtime`: "did not match any packages").
- Completing a turn commits no span: `TurnCommands` has no span command at all, and the unit's Check 1 holds.
- Turn ids come from the store (`turn_policy.rs:239`). An invented id would turn the unit's Contested case into `Unknown`.
- `stop` reaches the sink last: the unit's `logged()` asserts it. This is reasoned only, not mutated.
- Output sequences rise by 1 per envelope through `Emitter`.

**6. Paths written outside the worktree** (all in `<unit scratch>/adversary-1/`)
- `red-run.log`, `suite.log`, `clippy.log`
- `mutant-M1-no-end-at-stop.log`, `mutant-M2-threshold-strict.log`, `mutant-M3-floor-samples.log`, `mutant-M4-no-causation.log`
- `run-mutant.sh`
- `mutant/` (copy and its `target/`): deleted

**7.**
```findings
- file: crates/inference-runtime/tests/adversary1_endpoint_controller.rs
  line: 384
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a run drained mid-utterance ends Completed with its turn still Open because the detector and turn policy end on a closed input edge without ending speech or completing the turn (vad.rs:292, turn_policy.rs:277)
- file: crates/inference-pipeline/src/nodes/turn_policy.rs
  line: 238
  category: judgement
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: an OpenTurn refused run-not-active, the outcome ess declares for a Draining run, fails the node and so the drain; no store in the tree wires that refusal yet
- file: crates/inference-pipeline/src/nodes/vad.rs
  line: 60
  category: contract-drift
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: envelope kinds speech_started and speech_ended name no event declared in ess, which says an envelope kind is the declared event name
- file: crates/inference-pipeline/src/nodes/vad.rs
  line: 20
  category: judgement
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the docs say speech going at end of input ends just past the last frame but Detector::end returns the start of a quiet run in progress
- file: crates/inference-pipeline/src/nodes/vad.rs
  line: 295
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: removing the end of speech at stop survives the unit suite; the new end-of-input case kills it
- file: crates/inference-pipeline/src/nodes/vad.rs
  line: 156
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: changing >= to > on the threshold survives the unit suite; the new threshold case kills it
- file: crates/inference-pipeline/src/nodes/vad.rs
  line: 95
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: changing the rounded-up sample count to floor division survives the unit suite; the new rounding case kills it
- file: crates/inference-pipeline/src/nodes/turn_policy.rs
  line: 265
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: dropping the turn event's causation_id survives the unit suite; the new cause case kills it
```