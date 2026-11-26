---
format: aep.planning-md/3
id: review-result:tests-under-contention-adversary-1
kind: review-result
status: active
title: Adversary pass over the tests under contention (2026-11-26)
relations:
- reviews: story:ticker-test-holds-under-contention
- reviews: story:nova-sonic-tests-wait-for-their-event
revision: 1
---
unit: T, story:ticker-test-holds-under-contention + story:nova-sonic-tests-wait-for-their-event. Findings cover ba6edcc7c on wave2/tests-under-contention, worktree <worktree>
verdict: NEEDS-CHANGE (2 warnings, 2 notes, 0 blockers)
cases: executed 798→828, red 6 (3 are findings; 2 are base-ticker copies that are meant to be red; 1 is an unrelated auth flake)
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory, <scratch>/evaluate/wave2-tests-adversary/ (listed in part 6)
needs-coordinator: F1 asks for a decision: the half rule trades one flake for another, and the fix is a choice of rule, not a one-line change

## 1. Diff
`git --no-pager diff --stat` is empty and `git status --short` is empty now. While the cases ran, the tree held only 7 untracked test files, no tracked change and no implementation file:
- adapters/inference-aws/tests/adversary3_t_wait_misses.rs
- adapters/inference-elevenlabs/tests/adversary3_t_ticker_load.rs
- adapters/inference-elevenlabs/tests/adversary3_t_blocks_base.rs (a copy of adversary_w2_ticker_blocks.rs on the base ticker)
- adapters/inference-elevenlabs/tests/adversary3_t_blocks_mut34.rs (a copy on a mutant ticker, plus the 100 ms case)
- adapters/inference-elevenlabs/tests/adversary3_t_blocks_head_floor.rs (a copy on the head ticker, plus the 100 ms case)
- adapters/inference-elevenlabs/tests/support/ticker_base.rs (the ticker as of 509af7728)
- adapters/inference-elevenlabs/tests/support/ticker_mut34.rs (the head ticker with `wall / 2` changed to `wall / 4 * 3`)

All 7 are saved as `added-cases.patch` (`git apply --check` passes), then removed. No file under attack was mutated, and nothing was planted in place.

## 2. Cases added, each run alone first

**adversary3_t_wait_misses.rs (aws).** A provider sends the steps of `unreadable_nova.rs::a_completion_end_without_its_stop_reason_ends_the_response` and then ends its stream. This stands in for a binding whose reader quits after a completionEnd without its stop reason.
- Head form (`completed()`, then the test's two checks): RED, verbatim: `panicked at adapters/inference-aws/tests/adversary3_t_wait_misses.rs:151:5: the session ends right after the completion, and the test's checks hold: [SpeechStarted, InputCommitted, InputTranscribed, ResponseCreated, OutputTranscriptDelta("Hello"), ResponseCompleted]`
- Base form (`drain`): green. The check `state == Open` sees the end of the session.
- Run 10 times: 10/10 the same result.

**adversary3_t_ticker_load.rs (elevenlabs).** The same `Contended` load as the unit's file (8 spinners pinned beside the test). A session computes 15 ms of its own processor time (schedstat field 1) between two awaits, which is under the 50 ms bound by more than three times.
- Head ticker beside the load: RED, verbatim: `a session that computed 15ms of its own processor time took 169.905103ms beside 8 spinners, and the ticker's largest gap is 85.030504ms: the bound fails on the load, not on a hold`
- Over 10 runs: red 10/10, gaps 70 to 118 ms.
- Base ticker, same load: green, gap 20.08 ms.
- Head ticker, no spinners: green 9/10. It was red once on the build node from host load alone: `free: the compute took 113.80225ms, largest gap 60.023279ms`.
- The first test of async_handles (20 syntheses) beside the same load: green 10/10 on both tickers. One run gave head 30.66 ms against base 11.39 ms, so the margin to the bound shrank from about 39 ms to about 19 ms.

**adversary3_t_blocks_mut34.rs** (the 7 cases of the unit's file plus a 100 ms busy-loop case, on the ticker with a ¾ cap). All 7 original cases stay GREEN on the mutant: busy loop 67.5 ms, yields 52.5 ms. The 100 ms case is RED, verbatim: `a busy loop held the runtime's thread Hold { held: 169.902153ms, ready: 150.013674ms } and the ticker's largest gap is 42.514912ms: under the bound, the two tests pass`
- On the head ticker (adversary3_t_blocks_head_floor.rs), the 100 ms case is green: `100 ms busy loop: Hold { held: 100.000594ms, ready: 79.999341ms }; largest gap 53.66026ms`.

**adversary3_t_blocks_base.rs** (the unit's 7 cases on the base ticker). The busy loop and yields are red (30.05 ms and 11.32 ms), which matches the implementor's report. The sleep, park, lock and read cases are green on the base ticker beside the load (sleep gap 200.23 ms).

## 3. Suite run, after the cases existed
Command: `cd <worktree> && cargo nextest run -p <pkg> --all-features`, one package at a time.
- inference-elevenlabs: `Summary [13.838s] 238 tests run: 233 passed, 5 failed, 5 skipped`, exit 100.
  - Failed, mine: `adversary3_t_ticker_load the_ticker_passes_a_15_ms_compute_beside_the_load` (F1), `adversary3_t_blocks_mut34 a_thread_computing_for_100_ms_beside_the_load_fails_the_tests` (F3).
  - Failed, base copies meant to be red: `adversary3_t_blocks_base a_thread_computing_for_200_ms_fails_the_tests` and `adversary3_t_blocks_base a_thread_yielding_in_a_loop_for_200_ms_fails_the_tests`.
  - Failed, not mine: `auth a_handshake_refused_with_401_is_a_typed_refusal_without_the_key`, at auth.rs:64 `left: None right: Some("fixture-key-…")`. The fixture server had not recorded the key yet. auth.rs is not in this unit and was last changed in 2937744e5. I did not run it at the base, so this is not a finding.
- inference-aws: `Summary [4.466s] 138 tests run: 137 passed, 1 failed, 3 skipped`, exit 100. The one failure is the head case of adversary3_t_wait_misses (F2).
- inference-cartesia: `47 tests run: 47 passed, 1 skipped`, exit 0.
- inference-testkit: `405 tests run: 405 passed`, exit 0.

Loops, each through the pool, all green 10/10:
- elevenlabs adversary_w2_ticker_blocks, alone.
- elevenlabs async_handles, alone.
- testkit async_handles, alone.
- elevenlabs ticker file beside async_handles and adversary_async_mid_input in one run with 8 test threads.
- Each of the 8 aws files alone.

No changed test went red. The slowest added case took 1.1 s.

## 4. Findings

**F1 (warning, NEEDS-CHANGE, introduced, boundary).** File adapters/inference-elevenlabs/tests/support/ticker.rs:85, `wall.saturating_sub(ready.min(wall / 2))`.
- Input: a session computes 15 ms of its own processor time between two awaits, on a processor shared with other runnable threads.
- What was measured: the head ticker counts that gap at 70 to 118 ms, red 10/10 beside the unit's own 8 spinners. It was also red once in 10 runs with no spinners at all, from build-node load (113.8 ms wall, 60.0 ms gap). The base ticker counts 20 ms under the same load.
- With a share of 1/k of a processor, any burst of about 100/k ms of the thread's own processor time now fails the bound. With 8 spinners that is about 11 ms.
- The control case (adversary_w2_ticker_blocks.rs:276) holds `|| {}`, so it cannot see this. The claim "a gap of the host alone still passes the bound up to 100 ms of wall time" is true, but under contention 100 ms of wall time is about 11 ms of work.
- What reaches it: the forge runs `task test-run` on ubuntu-latest (4 processors) with test-threads 8. The 7 cases of adversary_w2_ticker_blocks each pin 8 spinners for about 0.7 s, and async_handles (the next binary in the order) and the cartesia and testkit ticker tests can run beside them. It also happens on a loaded build node without any spinners.
- Not shown red on the real async tests: 0 failures in 40 runs, but the margin of 20 syntheses beside the load fell from about 39 ms to about 19 ms.
- Fix: make the control case compute a share under the bound (for example 15 ms) beside the same load and require it to pass. Then pick a rule that passes it and still fails the 200 ms forms. For example, count only the ready time that lies outside the thread's own runs, or keep the full subtraction and pin the sleep form with a check of the kernel count. This is a design choice.

**F2 (warning, NEEDS-CHANGE, introduced, mutant).** File adapters/inference-aws/tests/unreadable_nova.rs:328 (`completed`), with the check at :336 (`state == Open`).
- The binding records an arrival into the ledger only when the caller takes the next event (src/duplex.rs:1366-1396). Read right after `completed()`, the session state is the state before any later arrival.
- So a session that ends right after a completionEnd without its stop reason passes the test now. The base `drain` caught it. Evidence: the adversary3_t_wait_misses cases above.
- The test exists for "keeps its meaning and the session stays open", so its second check can now fail only when the failure comes with the completion's own arrival.
- What reaches it: a reader that returns on the missing field, which is the defect class the module doc names.
- Fix: after `completed`, keep a quiet `drain` with the one-line comment the story asks for ("nothing ends the session after the completion") and then check the state.
- Other `until` sites I checked keep their absence checks covered: usage_is_a_running_total_per_response through the records count, counted_once, the barge-in test, and the tool tests.

**F3 (note, CONFIRMED, introduced, mutant).** File adapters/inference-elevenlabs/tests/support/ticker.rs:85.
- The documented floor ("a hold of 100 ms or more fails the bound whatever the field counted") has no case.
- A ¾ cap passes all 7 cases of the unit's file, with yields at 52.5 ms. The added 100 ms busy-loop case kills the mutant (42.5 ms) and passes on head (53.7 ms).
- Fix: add the 100 ms case from added-cases.patch (adversary3_t_blocks_head_floor.rs, last case) to adversary_w2_ticker_blocks.rs.

**F4 (note, CONFIRMED, introduced, judgement).** File adapters/inference-elevenlabs/tests/adversary_w2_ticker_blocks.rs:159, `largest >= hold.held.saturating_sub(hold.ready)`.
- The check holds by arithmetic for any ticker that takes out no more than the thread's ready time, because the Reading brackets keep the ready time inside the wall time. That includes the base ticker, so the check cannot fail.
- The story names the asleep case, and that case is green on the base ticker beside the same load (gap 200.23 ms). On this kernel only the busy-loop and yields cases separate head from base. The asleep case's red exists only in the implementor's planted scratch copy.
- Fix: say so in the doc, or replace the check with one that the old full subtraction fails.

## 5. Attacked, not broken
- taskset on the forge: ubuntu-latest has util-linux. Where taskset is missing, the case panics at `.expect("taskset runs")` or the `assert!` (red, not a skip).
- Spinners left after a failure: `Drop` joins them on unwinding, and each test is its own process under nextest. `cargo test` runs only `--doc`.
- Ready time above wall time: the Reading brackets keep the ready time inside the wall time, and `min(wall/2)` caps it.
- A hold of 60 to 99 ms, or two holds of 60 ms that stay ready to run: they pass on a contended processor, and the doc states that limit. Not raised.
- Nova Sonic waits: all 8 files were green 10/10 alone.
- `until_within` bounds: 3 s of empty waits or 20 s in all, and `drain` is at most about 50 s. Only `after_sent` (20 s plus a drain of up to about 50 s) can pass 60 s, and only if events keep coming just under 100 ms apart for 50 s. No caller does that, so I raised no finding.
- A second settle of usage in usage_is_a_running_total_per_response: the `[Some(first), Some(second)]` records check still catches it.

## 6. Paths written outside the worktree
All under <scratch>/evaluate/wave2-tests-adversary/:
- added-cases.patch
- loops.sh, floor-case.rs.txt
- aws-case-red.log, el-case-red.log
- adversary3_t_blocks_head_floor.log, adversary3_t_blocks_mut34.log, adversary3_t_blocks_base.log
- suite-inference-elevenlabs.log, suite-inference-aws.log, suite-inference-cartesia.log, suite-inference-testkit.log
- loops/ (summary.txt and 130 run logs)

```findings
- file: adapters/inference-elevenlabs/tests/support/ticker.rs
  line: 85
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "The half rule fails the 50 ms bound for a session that computes only 15 ms between awaits on a contended processor (gaps 70-118 ms beside the unit's own spinners, red 10/10, and once with host load alone), where the base ticker counts 20 ms."
- file: adapters/inference-aws/tests/unreadable_nova.rs
  line: 328
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "Waiting only for the completion means the test's state == Open check no longer sees a session that ends right after a completionEnd without its stop reason, which the replaced drain caught."
- file: adapters/inference-elevenlabs/tests/support/ticker.rs
  line: 85
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The documented floor that any hold of 100 ms or more fails has no case: a three-quarter cap passes all seven cases of the unit's file and only an added 100 ms busy-loop case kills it."
- file: adapters/inference-elevenlabs/tests/adversary_w2_ticker_blocks.rs
  line: 159
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The counted-in-full check holds by arithmetic for any ticker that subtracts at most the ready time, so the story's own asleep case is green on the base ticker beside the same load and cannot tell base from head."
```
