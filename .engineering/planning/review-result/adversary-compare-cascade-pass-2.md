---
format: aep.planning-md/3
id: review-result:adversary-compare-cascade-pass-2
kind: review-result
status: active
title: Adversary pass 2 on story:compare-elevenlabs-cascade-and-ollama
relations:
- reviews: story:compare-elevenlabs-cascade-and-ollama
revision: 1
---
```
unit: story:compare-elevenlabs-cascade-and-ollama pass 2 (fixes only), commit 8fe9ddea (cases on adv/compare-cascade-2 @ 97e806f5)
verdict: NEEDS-CHANGE
cases: executed 4040→4045, red 2
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 17 files, all in the assigned scratch dir
needs-coordinator: the worktree is back on impl/compare-elevenlabs-cascade @ 8fe9ddea and clean; the pass-1 binary cases need the one-line fix in part 4, which I may not apply (hard rule 2)
```

**1. `git --no-pager diff --stat 8fe9ddea adv/compare-cascade-2`**
```
 .../tests/adversary_compare_cascade_2.rs           | 571 +++++++++++++++++++++
 1 file changed, 571 insertions(+)
```
It is a single test file. No implementation path was changed.

**2. Cases added** in `examples/realtime-voice/tests/adversary_compare_cascade_2.rs`. This run of the file alone was the first time any of them executed. The binary cases set `DBUS_SESSION_BUS_ADDRESS` to a socket that does not exist, so they never reach the real keychain.

| case | asserts | now |
|---|---|---|
| `adv2_a_recognition_left_to_end_on_its_own_does_not_refuse_the_next_runs_turn` (:373) | A stall outlasts the limit and the drain, so the first run leaves its recognition running. The second run's turn should not be refused. | **red** |
| `adv2_an_arm_skipped_for_its_llm_does_not_ask_the_keychain` (:552) | `--cascade-stt elevenlabs` with no LLM variables (the arm cannot run) should not ask the OS keychain. | **red** |
| `adv2_a_stall_caught_in_the_drain_is_noted_once_and_stays_in_its_run` | Across 2 runs: exactly one "recognition failed" note per run, and no refusal. | green |
| `adv2_an_unwritable_runs_out_prints_the_table_then_fails_naming_the_file` | exit code 1, the table on stdout, the error naming `--runs-out` on stderr | green |
| `adv2_with_the_key_variable_set_the_keychain_is_not_asked` | `ELEVENLABS_API_KEY` set: no keychain on stdout or stderr | green |

Red output, verbatim:
```
---- adv2_a_recognition_left_to_end_on_its_own_does_not_refuse_the_next_runs_turn stdout ----
cascade: question run 1 of 2
cascade: question run 2 of 2
["no end of the answer within 2s", "the recognition had not ended 300ms after its turn was cut; it is left to end on its own"]
["the recogniser refused the turn: the recogniser already runs 1 recognition(s), its declared maximum concurrency"]
thread '...' panicked at examples/realtime-voice/tests/adversary_compare_cascade_2.rs:398:5:
the second run is refused by the first run's recognition, still running: ["the recogniser refused the turn: the recogniser already runs 1 recognition(s), its declared maximum concurrency"]

---- adv2_an_arm_skipped_for_its_llm_does_not_ask_the_keychain stdout ----
status ExitStatus(unix_wait_status(0))
stdout:
...
cascade   cascade   skipped: no hosted TTS configured: ELEVENLABS_API_KEY is not set and the OS keychain did not open: credential source is unavailable
thread '...' panicked at examples/realtime-voice/tests/adversary_compare_cascade_2.rs:567:5:
the keychain was asked for an arm without an LLM:

test result: FAILED. 3 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.84s
exit 101
```

**3. Suite, run after the cases existed**
- `slot task check`: exit 201. The ess, aep, docs-check, fmt, clippy `-D warnings` and doc steps all passed with my file in the tree. The test step stopped fail-fast at `crates/inference/tests/adversary2_agent.rs:124` (`frames_fed_while_a_close_drains_do_not_fail_the_device_run`), the same flaky case as in pass 1. It is outside this unit and passed in the next run.
- `cargo test --workspace --all-features --no-fail-fast`: exit 101, 4020 passed, 2 failed (my two reds), 63 ignored.
- `cargo test -p inference --no-default-features --test stack`: exit 0, 23 passed.
- Executed: 4022 + 23 = 4045, which is 4040 plus my 5.

**4. Findings** (covering commit 8fe9ddea)

| # | file:line | verdict | origin | what was measured | what reaches it |
|---|---|---|---|---|---|
| G1 | `examples/realtime-voice/src/compare.rs:1883` (with `:1909`, `:1849`) | NEEDS-CHANGE | introduced | Red case :373. `release_listener` gives up after `plan.drain`, but the recognition it leaves still holds `ElevenLabsRecogniser`'s only slot (max concurrency 1) until its read timeout passes. The next run's `recogniser.start` is refused, so one stall costs two runs, and the second run's note blames concurrency. | With `compare`'s fixed values (read timeout 10 s, limit 20 s, drain 3 s), any `--clips` question longer than about 12.4 s reaches it, and so does the next clip. Fix: when the next turn starts, wait for the leftover listener (or cancel its recognition) before calling `start`. Alternatively, make the drain at least the endpoint's read timeout. |
| G2 | `examples/realtime-voice/examples/compare.rs:335` | CONFIRMED | introduced | Red case :552. The key is resolved before anything else the arm needs is checked: before the LLM in `--cascade-stt elevenlabs`, before `cartesia()` and before the local STT config. So an arm that is going to be skipped still asks the OS keychain, which can mean an unlock prompt and up to 8 s waiting. In a build without feature `local`, the default provider list includes `cascade`, so plain `cargo run --example compare` now asks the keychain for an arm that can never run (this follows from reading the code; that build isn't part of the suite). | Any `compare` run without `ELEVENLABS_API_KEY` whose cascade arm is missing anything else. Fix: check the cheap prerequisites first (the `local` build, `generator()`, the local config, `cartesia()`) and resolve the key last. |
| G3 | `CHANGELOG.md:28` | CONFIRMED | introduced | `run_cascade`, `run_cascade_live` and `measure_cascade` now require `R::Handle: Send + 'static` (`src/compare.rs:1493`, `:1512`, `:1532`). The changelog's breaking entry for these functions doesn't mention the new bound. | Every caller in the repo already passes `'static` handles, so the only gap is the changelog. |
| T1 | `examples/realtime-voice/tests/adversary_compare_cascade_1.rs:473` | CONFIRMED | introduced | My pass-1 helper `compare()` clears the environment, so `key()` falls through to the real OS keychain. | A test should never do that: the result then depends on the machine's secrets and keychain lock state, it can raise a prompt or wait 8 s, and it pulls a real secret into a test process. The smallest fix is one line in that helper: `.env("ELEVENLABS_API_KEY", "fixture-key-not-a-real-key")`. `key()` then takes the env path on every OS, and `hosted()` skips for the missing voice with no network. Only when the keychain ask itself is under test, point `DBUS_SESSION_BUS_ADDRESS` at a dead socket, as pass 2 does (Linux only). I wrote that file, but it is now an existing case, so hard rule 2 means I name the fix and don't apply it. |

**5. Attacked, could not break**
- **A late failure landing in the next run's notes:** can't happen. Each run makes its own channel (`src/compare.rs:1604`), so a detached worker's report goes to a dropped receiver. Inside a run, the turn is only bumped after the release, so stale turns are still dropped.
- **Duplicate notes:** a failure caught in the drain is noted once (green case).
- **Thread leak:** each detached worker ends within the endpoint's read timeout or connect timeout plus `OPEN_SLACK`, so leftover threads stay bounded. The harm is G1, not the leak.
- **The `'static` bound:** it compiles for every in-repo caller, the pass-1 helpers included, because a projection is `'static` when `R` is.
- **F2:** the table is printed first, then the error goes to stderr with exit code 1. When the write succeeds, exit is 0.
- **F3, keychain read twice or with the env var set:** `key()` is resolved once per cascade entry, and skip is checked before it. With `ELEVENLABS_API_KEY` set the keychain is never read (green case). A key that is set but empty is refused without the keychain. It is resolved twice only with `--providers cascade,cascade`, which is contrived.
- **F4:** the notice now goes through `eprintln!` (`src/elevenlabs.rs:100`). The real keychain can't be reached from a test, so it isn't observed by a case.

**6. Paths written outside the worktree**
All 17 are in the assigned scratch dir `.../p2/wave1/adv/` named in the brief.
- From pass 1: `adv-alone.log`, `build-adv.log`, `build-compare.log`, `clippy-nolocal.log`, `commit-msg.txt`, `help-bin.txt`, `help-doc.txt`, `task-check.log`, `test-nofailfast.log`, `test-stack.log`.
- New in pass 2: `pass2-head.rs`, `pass2-tail.rs`, `pass2-full.rs`, `adv2-alone.log`, `suite2.sh`, `suite2.done`, `commit-msg-2.txt`, `task-check-2.log`, `test-nofailfast-2.log`, `test-stack-2.log`.
- Inside the worktree, the runs also used `target/tmp/adv2-*`.

**7. Findings block**
```findings
- file: examples/realtime-voice/src/compare.rs
  line: 1883
  category: boundary
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a recognition left to end on its own after the drain still holds the ElevenLabs recogniser's only slot, so the next run or clip is refused with a concurrency note, reachable with a --clips question over about 12.4 s
- file: examples/realtime-voice/examples/compare.rs
  line: 335
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the ElevenLabs key is resolved before the arm's LLM, local build and other steps are checked, so an arm that will be skipped still asks the OS keychain, which can prompt and wait up to 8 s
- file: CHANGELOG.md
  line: 28
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the new R::Handle Send + 'static bound on run_cascade, run_cascade_live and measure_cascade is missing from the changelog's breaking entry
- file: examples/realtime-voice/tests/adversary_compare_cascade_1.rs
  line: 473
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the pass-1 binary helper clears the environment and so reaches the real OS keychain; setting ELEVENLABS_API_KEY to a fixture value in that helper keeps it hermetic
```