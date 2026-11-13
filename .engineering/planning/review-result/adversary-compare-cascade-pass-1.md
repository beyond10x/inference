---
format: aep.planning-md/3
id: review-result:adversary-compare-cascade-pass-1
kind: review-result
status: active
title: Adversary pass 1 on story:compare-elevenlabs-cascade-and-ollama
relations:
- reviews: story:compare-elevenlabs-cascade-and-ollama
revision: 1
---
```
unit: story:compare-elevenlabs-cascade-and-ollama pass 1, commit 46e3a100 (cases on adv/compare-cascade-1 @ 540c9474)
verdict: NEEDS-CHANGE
cases: executed 4034→4040, red 2
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 10 files, all in the assigned scratch dir
needs-coordinator: the worktree HEAD is now on adv/compare-cascade-1 (one commit on top of 46e3a100, the implementor's branch is untouched); unrelated flaky case crates/inference/tests/adversary2_agent.rs:124
```

**1. `git --no-pager diff --stat 46e3a100 HEAD`**
```
 .../tests/adversary_compare_cascade_1.rs           | 543 +++++++++++++++++++++
 1 file changed, 543 insertions(+)
```
It is a single test file. No implementation path was changed.

**2. Cases added** in `examples/realtime-voice/tests/adversary_compare_cascade_1.rs`. This run of the file alone was the first time any of them executed: `cargo test -p realtime-voice --features local --test adversary_compare_cascade_1`.

| case | asserts | now |
|---|---|---|
| `adv_a_stall_past_the_runs_limit_is_still_a_note_of_the_arms_run` (:402) | A commit is never answered and the read timeout passes after `plan.limit`. The run's notes should still name the recognition's failure. | **red** |
| `adv_a_runs_out_file_that_does_not_write_does_not_swallow_the_table` (:518) | `compare --providers cascade --cascade-stt elevenlabs --runs-out <missing dir>/runs.jsonl` should still print the table. | **red** |
| `adv_a_stall_within_the_limit_is_a_note_of_the_arms_run` | a stall inside the limit becomes a note | green |
| `adv_a_connection_closed_at_the_commit_is_a_note_of_the_arms_run` | a close without a commit becomes a note | green |
| `adv_the_first_audio_bound_catches_an_arm_that_waits_out_the_vad_quiet_window` | The same arm with VAD commit (`elevenlabs::recogniser`) gives first audio above hold + delays + 250 ms, so the acceptance bound catches it. | green |
| `adv_compare_prints_its_table_with_a_writable_runs_out` | control for the runs-out case | green |

Red output, verbatim (worktree path shortened):
```
---- adv_a_runs_out_file_that_does_not_write_does_not_swallow_the_table stdout ----
status ExitStatus(unix_wait_status(256))
stdout:

stderr:
INFERENCE_COMPARE_BARGE_IN_WAV is not set: no barge-in is sent
error: <worktree>/target/tmp/adv-no-such-dir-2819041/runs.jsonl was not written: No such file or directory (os error 2)
thread '...' panicked at examples/realtime-voice/tests/adversary_compare_cascade_1.rs:538:5:
the comparison ran and its table was not printed because --runs-out did not write; stdout:

---- adv_a_stall_past_the_runs_limit_is_still_a_note_of_the_arms_run stdout ----
cascade: question run 1 of 1
Measured { topology: Cascade, clip: "question", first_audio: None, ... notes: ["no end of the answer within 3s"] }
thread '...' panicked at examples/realtime-voice/tests/adversary_compare_cascade_1.rs:407:5:
the recogniser's stall is dropped from the arm's row; its notes: ["no end of the answer within 3s"]

test result: FAILED. 4 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.72s
exit 101
```

**3. Suite, run after the cases existed**
- `slot task check`: exit 201. The ess, aep, docs-check, fmt, clippy `-D warnings` and doc steps all passed with my file in the tree. The `cargo test --workspace --all-features` step stopped at my target (fail-fast) after 3844 passed and 2 failed (my two reds).
- For the count, `cargo test --workspace --all-features --no-fail-fast`: exit 101, 4014 passed, 3 failed, 63 ignored. The failing targets were mine and `-p inference --test adversary2_agent`.
- `cargo test -p inference --no-default-features --test stack`: exit 0, 23 passed.
- Executed: 4014 + 3 + 23 = 4040, which is 4034 plus my 6.
- The third failure, `frames_fed_while_a_close_drains_do_not_fail_the_device_run` (`crates/inference/tests/adversary2_agent.rs:124`), passed in the `task check` run on the same tree. It is outside this unit's diff and looks timing-flaky under load.

**4. Findings** (covering commit 46e3a100)

| # | file:line | verdict | origin | what was measured | what reaches it |
|---|---|---|---|---|---|
| F1 | `examples/realtime-voice/src/compare.rs:1963` (with `:1839`) | NEEDS-CHANGE | introduced | Red case :402. When the run's limit fires first, `cut()` bumps `turn` and then joins the listening worker, which is blocked in `next()` until the read timeout. Its `Heard::Failed` arrives with the old turn and is dropped, so the row says only "no end of the answer within 20s" and the run also hangs for up to 10 s after its limit. | The real values are fixed: `DEFAULT_READ_TIMEOUT` is 10 s and `Plan::limit` is 20 s from the question's start. Any `--clips` question longer than about 9.4 s reaches this. Neither value can be changed from the command line. The new `failure` path only exists in this unit. Fix: put the recognition's failure in the run's notes when it is cut, for example by draining the reports of the old turn after the join, or by bounding the wait. |
| F2 | `examples/realtime-voice/examples/compare.rs:257` | NEEDS-CHANGE | introduced | Red case :518. When `--runs-out` cannot be written, `main` returns Err before `print!("{comparison}")`, so it exits 1 and prints no table. | Any mistyped `--runs-out` path after a billed multi-provider run loses all the printed results. Fix: print the table first and then report the write error, or check the file can be written before measuring. |
| F3 | `website/docs/guides/compare-voice-providers.md:67`, `website/docs/reference/environment.md:32` | CONFIRMED | introduced | The docs say the ElevenLabs arm's key is `ELEVENLABS_API_KEY` or the OS keychain's. With the default `--cascade-voice elevenlabs`, `hosted()` (`examples/compare.rs:448`) reads only `EnvironmentSource`, so without the variable the arm is skipped before `elevenlabs::key()` asks the keychain. | Following the guide with only a keychain key gives "no hosted TTS". The keychain is only reached with `--cascade-voice cartesia`. No case written: the keychain can't be controlled from a test. |
| F4 | `examples/realtime-voice/src/elevenlabs.rs:99` | CONFIRMED | introduced | `key()` moved into the library and still uses `println!`. In `compare`, stdout is the table and progress goes to stderr, so the keychain notice ends up inside a redirected table. | `compare --cascade-voice cartesia --cascade-stt elevenlabs` without `ELEVENLABS_API_KEY`. Fix: `eprintln!`. |

**5. Attacked, could not break**
- The 250 ms bound is real: a VAD-commit arm fails it (green case above). The hold is not counted twice, because End is sent exactly one hold after the `QuestionEnd` frame, and the commit goes out on `end_input`.
- A stall inside the limit, a close without a commit and an auth error all become notes. An error after the commit is never read because the recognition has already finished, so there is nothing to lose.
- p95 by nearest rank is correct for 1, 2, 3, 10 and 20 runs.
- `runs_json`: one valid line per run, every step that applies is present, skipped arms are left out.
- `--clips`: an empty directory and a broken WAV are refused and named; other files, directories and `.wav` dot-files are left out; stereo is averaged; other rates are resampled; order is by path.
- `talk`: every `Refused` still exits 2. The loopback check in `generator()` refuses nothing that `compare` accepted before, because `LocalModels::open` already required loopback.
- Without feature `local`, `cargo clippy -p realtime-voice --all-targets -D warnings` exits 0. `task check` only builds with `--all-features`.
- The guide's `--help` block is identical to the binary's `--help` output.

**6. Paths written outside the worktree**
All 10 are in the assigned scratch dir `.../p2/wave1/adv/` named in the brief: `adv-alone.log`, `build-adv.log`, `build-compare.log`, `clippy-nolocal.log`, `commit-msg.txt`, `help-bin.txt`, `help-doc.txt`, `task-check.log`, `test-nofailfast.log`, `test-stack.log`. Inside the worktree, the runs also used `target/tmp/adv-*`.

**7. Findings block**
```findings
- file: examples/realtime-voice/src/compare.rs
  line: 1963
  category: boundary
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a recogniser stall whose read timeout passes after the run's limit is reported under the cut turn and dropped, so the arm's row says only "no end of the answer within 20s", reachable with a --clips question over about 9.4 s
- file: examples/realtime-voice/examples/compare.rs
  line: 257
  category: boundary
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a --runs-out file that cannot be written makes compare exit 1 without printing the table of the runs it already measured and billed
- file: website/docs/guides/compare-voice-providers.md
  line: 67
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the guide promises the ElevenLabs arm takes its key from the OS keychain, but with the default --cascade-voice elevenlabs the hosted TTS reads only ELEVENLABS_API_KEY and skips the arm first
- file: examples/realtime-voice/src/elevenlabs.rs
  line: 99
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: key() now lives in the library and prints its keychain notice to stdout, which in compare is the table's stream
```