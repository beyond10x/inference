---
format: aep.planning-md/3
id: review-result:test-run-step-keeps-its-margin-adversary-1
kind: review-result
status: active
title: Adversary pass over the margin of the test step (2026-11-26)
relations:
- reviews: story:test-run-step-keeps-its-margin
revision: 1
---
unit: U2 story:test-run-step-keeps-its-margin, worktree inference-wave-u2 at 68d199eca (branch wave/u2-test-margin)
verdict: findings: 1 warning (CONFIRMED, red case), 3 notes (1 INFEASIBLE, 2 CONFIRMED)
cases: executed 5→7 (binary realtime-voice::adversary_compare_cascade_2), red 1
origin: introduced 3 / pre-existing 1 / undecided 0
wrote-outside-worktree: 1 directory, <scratch>/wave-1126/u2-adversary/ (added-cases.patch, case-alone.log, suite-after.log, suite-before.log, dry-check.txt, synth.log), plus the build node's target/ of this worktree's mirror (slot synced it to the build node)
needs-coordinator: whether to widen the stall test's commit-before-limit margin before the 8-thread forge run (fix below)

**1. Diff while the cases were in** (removed again; `git status --short` is empty now; the patch applies with `git apply --check`)
```
 .../tests/adversary_compare_cascade_2.rs           | 44 ++++++++++++++++++++++
 1 file changed, 44 insertions(+)
```
Test file only. Two cases appended, no existing case touched. No implementation file was edited.

**2. Cases added** (`examples/realtime-voice/tests/adversary_compare_cascade_2.rs`, saved in `added-cases.patch`)
- `adv_u2_a_stall_whose_commit_goes_out_150_ms_late_is_noted_once`: this is U2's stall control (read timeout 1 s, limit 1 s, drain 3 s) with the commit going out 150 ms later. The question is voiced for 450 ms instead of 300 ms, so the commit goes out near 1.05 s instead of 0.9 s. It asserts the same as the control: one "the recognition failed" note per run. **RED.** Run alone, before any other run:
  ```
  FAIL [   2.052s] (1/1) realtime-voice::adversary_compare_cascade_2 adv_u2_a_stall_whose_commit_goes_out_150_ms_late_is_noted_once
  thread '...' panicked at examples/realtime-voice/tests/adversary_compare_cascade_2.rs:599:9:
  assertion `left == right` failed: read timeout 1s, limit 1s: ["no end of the answer within 1s"]
    left: 0
   right: 1
  exit 100
  ```
- `adv_u2_a_stall_whose_commit_goes_out_150_ms_late_is_noted_once_under_the_values_before`: the same 150 ms late commit with the values before U2 (2 s and 2 s). **GREEN**, 6.2 s (over 5 s).

**3. Suite run, after the cases existed**
- Command: `cargo nextest run -p realtime-voice --all-features --test adversary_compare_cascade_2`
  - Summary [6.259s]: 7 tests run, 6 passed, 1 failed (the U2 case above). Exit 100.
- Before figure: the same command with `-E 'not test(/^adv_u2_/)'`, my two cases excluded.
  - 5 tests run, 5 passed, exit 0.

**4. Findings**

F1, warning, CONFIRMED, introduced. `examples/realtime-voice/tests/adversary_compare_cascade_2.rs:423` (and the read timeout at :416).
- What was measured: the case above.
  - The commit goes out when the 600 ms hold (`SPEECH_VAD.hold`) ends: 0.3 s + 0.6 s = 0.9 s after the turn starts.
  - `CascadeRunner::drive` calls `decide(now)` before `send` (`examples/realtime-voice/src/compare.rs` around 1760-1770). Once `turn_started + limit` has passed, the turn is cut before the commit frame goes out.
  - `listening` then sees its channel close without `Listen::End`, the recognition is dropped and no failure is reported. So the count is 0, not 1.
- The report's "same margins" holds only for the two margins after the commit (0.9 s and 2.1 s). The margin from commit to limit went from 1.1 s to 0.1 s.
- What reaches it: the driving thread kept off its processor for more than 100 ms near 0.9 s. That is CPU contention, which the coordinator's `test-threads = 8` on the 4-vCPU forge raises. I could not show it on the forge (not allowed).
- Fix: keep the limit at 2 s, or set the limit to 1.5 s with read timeout 1 s and drain 3 s. With 1.5 s the commit margin is 0.6 s, the timeout passes 0.4 s after the limit and 2.6 s before the drain ends. The test costs about 1 s more than U2's version.

F2, note, INFEASIBLE, introduced (exposed by 68d199eca). `crates/inference-testkit/tests/async_handles.rs:172` (also :221 and :300).
- What was read: `largest < GAP_BOUND`, 50 ms of wall time on a current-thread runtime, with no correction for time the thread waits for a processor.
- The elevenlabs copy (`adapters/inference-elevenlabs/tests/support/ticker.rs:96`) subtracts that wait (`/proc/thread-self/schedstat`). This one does not.
- With 8 test threads on 4 vCPUs, beside tests that start many threads (TUI 41, resampler 12-16, cascade_burst 8 workers), a 50 ms pause of the thread is plausible.
- What reaches it: only a forge run under `test-threads = 8`, not run (brief).
- Fix: use the corrected ticker of `support/ticker.rs` here too.

F3, note, CONFIRMED, pre-existing. `adapters/inference-elevenlabs/tests/adversary_w2_ticker_blocks.rs:77`.
- What was measured: the implementor's run 1 (u2-report.md). `a_thread_asleep_for_200_ms_fails_the_tests` failed with "ticker's largest gap is 11.2 ms" at load 86. When CPUs are contended, the time it subtracts swallows the 200 ms block.
- What reaches it: the same CPU contention, which 8 threads on 4 vCPUs makes more likely.
- Fix: a story under "Tests stay fast"; not U2's diff.

F4, note, CONFIRMED, introduced. `AGENTS.md:43`.
- The line describes the profiles of `.config/nextest.toml` but does not say that `default` now runs 8 tests at once (`.config/nextest.toml:5`, commit 68d199eca). It also does not say that slot's `NEXTEST_TEST_THREADS` overrides that number.
- `website/docs/guides/run-the-checks.md` does not say it either.
- Fix: one clause in each page.

Other claim-3 candidates, read only, not raised as findings:
- `examples/realtime-voice/tests/compare_elevenlabs.rs:418` (first audio within hold + delays + 250 ms)
- `adapters/inference-openai/tests/stall_bound.rs:173` (READ_TIMEOUT + 250 ms)
- `adapters/inference-deepgram/tests/agent_duplex.rs:476` and `adapters/inference-elevenlabs/tests/agent_duplex.rs:481` (fixed ms + CLOSE_BOUND)
- These are upper bounds on wall time that the forge's contention can cross. Their slack is at least 250 ms.

Commit and tree covered: 68d199eca, clean worktree.

**5. Attacked and not broken**
- usage_view cut (Claim 1):
  - The mutant rewrites only `query_view` results for `inference.usage.UsageRecords`.
  - Every view read in `ess_conformance` 0.98.0 (`runner.rs:746`, `:1209`, through `read()` at `:1624`) comes from a scenario step that names the view. So no scenario outside the cut can see a mutant.
  - The synthesized suite has 491 scenarios, 3 of which contain the name.
- Resampler threads: inputs, the order of draws from the one generator, and the assertions are identical to the base (compared against `git show 21bf7bb91:`).
  - A panic in a pair thread reaches the test through `resume_unwind`. A thread that never ends hangs the test as the serial loop did, and nextest's 60 s kill applies.
  - `inference-audio/src` has no global state that a thread could share.
- TUI threads: same widths and heights; a panic in a width thread propagates.
- cascade_burst: `synthesize(&FIXTURE)` is deterministic (no random number, no clock), so one fixture equals 960.
- Claim 2 setup lane:
  - The cache key is the SHA-256 of every file under `ess/`, of `ess --version` and of the added witness scenarios (`suite/mod.rs:134`). A stale suite is never read.
  - The existing case `a_suite_is_synthesized_again_only_for_another_source_version_or_added_scenario` covers that.
  - `test-run` without `test-build` synthesizes under the lock as before.
- Docs: the `task --dry check` listing in `run-the-checks.md` matches the real output, apart from the archive lines, which are up to date in the documented tree.

**6. Paths written outside the worktree**
- <scratch>/wave-1126/u2-adversary/ (added-cases.patch, case-alone.log, suite-after.log, suite-before.log, dry-check.txt, synth.log; a 12 MB synthesized suite I made there to inspect was deleted)
- The build node's target/ of this worktree's slot mirror (the build node): test build of realtime-voice with --all-features

**7. Findings block**
```findings
- file: examples/realtime-voice/tests/adversary_compare_cascade_2.rs
  line: 423
  category: boundary
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: "The limit cut to 1 s leaves the stall control's commit (0.9 s) only 100 ms inside it, and a commit 150 ms late turns the test red with no failure note, where the 2 s values stay green."
- file: crates/inference-testkit/tests/async_handles.rs
  line: 172
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: "The 50 ms wall-time gap bound is not corrected for processor wait as the elevenlabs ticker is, and test-threads = 8 on the 4-vCPU forge makes a 50 ms pause plausible; not shown without a forge run."
- file: adapters/inference-elevenlabs/tests/adversary_w2_ticker_blocks.rs
  line: 77
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: "The test already failed once under load (largest gap 11.2 ms at load 86), and 8 threads on 4 vCPUs raise that contention."
- file: AGENTS.md
  line: 43
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The nextest profile description omits that the default profile now runs 8 tests at once and that slot's NEXTEST_TEST_THREADS overrides it."
```
