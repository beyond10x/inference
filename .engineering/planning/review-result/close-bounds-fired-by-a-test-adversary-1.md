---
format: aep.planning-md/3
id: review-result:close-bounds-fired-by-a-test-adversary-1
kind: review-result
status: active
title: Adversary pass over the tests that fire a close bound (2026-11-25)
relations:
- reviews: story:close-bounds-fired-by-a-test
revision: 1
---
unit: story:close-bounds-fired-by-a-test, worktree <worktree> at 08c0f5bc5 (commits edeb51720, 08c0f5bc5 on base eb32eb8b2)
verdict: red (findings: 0 errors, 2 warnings, 7 notes; no failing case added)
head: 08c0f5bc5, unchanged, not pushed
wrote-outside-worktree: only under <scratch> (listed at the end)
uncommitted-in-worktree: none
cases: no case added; suites ran 34 + 131 + 72 + 169 + 74 tests, the only red are the two known ticker tests
origin: introduced 8 / pre-existing 1 / undecided 0
needs-coordinator: two things I did that you should know of, under "Method"

`git --no-pager diff --stat` of the worktree now: empty. `git status --short`: empty. `git diff HEAD -- ':(glob)adapters/*/src/**'`: empty. Lease ended with `worktree hook session-end`.

## Method

- **Mutants in the tree, not in a copy.** The role file says to mutate a copy; your brief allows planted and restored mutants. I planted in the tree (13 files) and restored each from HEAD's content. Proof above and in `adversary-1/out/restore-1.txt` and the end of `adversary-1/out/final.log`.
- **One switchable mutant per crate.** The bound is read from `ADV_BOUND_MS`; each timeout, the close itself, the fixture's deafness and the flood are switched off by one variable each. With no variable set, all six tests pass (`*-unswitched.txt`). Patches: `adversary-1/patch/`.
- **The constant itself at 3 s**, your implementor's plant, repeated for all four crates (`literal-3s-*.txt`).
- **Base comparison, outside the letter of your brief.** For Phonic I put the base content of `tests/duplex.rs` and `tests/support/mod.rs` in place, ran the suite, and restored both. `duplex.rs` holds the new test, which your brief says I do not edit.

## Findings

1. **warning, introduced, INFEASIBLE (reached only by my plant).** Against a fixture that reads on, the two transcription tests pass in some runs.
   - Ran: `ADV_NOT_DEAF=1 cargo nextest run -p <crate> --all-features --test … -E 'test(on_a_backed_up_connection)' --no-capture`.
   - Deepgram transcription passed 1 of 4 runs: `a flood of 576000 pings, read within 3.839495923s: … the connection thread was gone after Some(1.013008951s); the server then read 500809 pongs, close frame: false`, `PASS [ 9.361s]`. The other three: `FAIL [ 7.565s]`, `[ 7.620s]`, `[ 7.157s]` at `cancel.rs:191`.
   - ElevenLabs speech to text passed 2 of 4: `PASS [ 9.951s]` (404,791 pongs) and `PASS [ 5.053s]` (547,339 pongs). The other two: `FAIL [ 5.114s]`, `FAIL [ 6.200s]` at `stt.rs:655`.
   - Cause: at 576,000 pings the client is still writing pongs when the cancel comes, so "fewer pongs than pings and no close frame" is true of a peer that never stopped reading. The bound did fire in those runs, so the limits were asserted of a real backed-up close.
   - With `ADV_BOUND_MS=3000` on top, both fail (`FAIL [ 7.083s]`, `FAIL [ 4.467s]`): no pass on an enlarged constant.
   - The four session tests fail in this case, once each: Phonic `FAIL [ 8.335s]`, ElevenLabs agent `FAIL [ 6.965s]` (both at the designed assertion), Deepgram agent `FAIL [ 10.757s]`, Deepslate `FAIL [ 11.894s]`.
   - The last two fail on another assertion: `the audio behind a flood of 576000 pings did not arrive: []`. `events_until` has 5 s (`agent_duplex.rs:98`, `opal_duplex.rs:124`) and 288,000 pings already took 3.1 s and 3.6 s. Their largest rung cannot be reached on the build node.
   - What reaches it: a fixture whose `Hold` or `deaf` stops holding, or a host whose buffers take 288,000 pongs (37.7 MB). On the build node nothing reaches it: 60 of 60 unplanted test runs backed up at 36,000 or 72,000.
   - `adapters/inference-deepgram/tests/cancel.rs:174`, `adapters/inference-elevenlabs/tests/stt.rs:638`. A fix would be for the fixture to report that it read no pong before `read_again`.

2. **warning, introduced, CONFIRMED.** `quality/adapters.md:70` says of the six tests "so it fails with the bound enlarged". A bound of 1,500 ms and of 1,900 ms passes all six.
   - Ran `ADV_BOUND_MS=1900` and `=1500` per crate. At 1,900: Phonic `held 1.901153374s` PASS, Deepgram agent `1.900849864s` PASS, Deepgram transcription `gone after Some(1.906516013s)` PASS, Deepslate `1.90114891s` PASS, ElevenLabs agent `1.911440831s` PASS, ElevenLabs speech to text `1.906787141s` PASS. At 1,500 all six pass too. Phonic at 1,990: `held 1.992195054s` PASS.
   - `quality/not-measured.md:343` ("The limits pass any bound from 1 s to under 2 s") is right about the upper side. The adapters.md sentence needs "to twice its value or more".

3. **note, introduced, CONFIRMED.** The lower limit admits a constant of 999 ms: Phonic with `ADV_BOUND_MS=999` gave `the close held 1.000308908s`, `PASS [ 1.297s]`. The page says "from 1 s" (`quality/not-measured.md:343`). `adapters/inference-phonic/tests/duplex.rs:510`.

4. **note, introduced, CONFIRMED.** The thread name the transcription tests match is not only the connection thread's.
   - `adapters/inference-deepgram/src/wake.rs:66` names the crate's clock thread `inference-deepgram-clock`; `adapters/inference-elevenlabs/src/wake.rs:63` `inference-elevenlabs-clock`; `adapters/inference-elevenlabs/src/synthesiser.rs:358` `inference-elevenlabs`. All share the 15 bytes of `cancel.rs:78` and `stt.rs:540`.
   - The clock starts when an async-form handle waits for a deadline and never ends. Neither test starts it: each asserts one thread before the cancel and reaches zero in every run. If it ran, the test would fail, not pass.
   - The names themselves are the source's (`recogniser.rs:623`, `recogniser.rs:758`).

5. **note, introduced, CONFIRMED.** Deepslate's first flood sits at the edge of what the build node's buffers take.
   - Ten runs: two rounds in 5 of 10, 1.388 s to 2.143 s. One first round counted 35,916 of 36,000 pongs. Second rounds counted 36,305 to 37,249.
   - The comment at `adapters/inference-deepslate/tests/opal_duplex.rs:1083-1085` says 36,000 pongs are more than the buffers hold. The test is right either way.

6. **note, introduced, CONFIRMED.** `closer.join()` has no bound of its own in the Deepgram agent test (`agent_duplex.rs:720`) and the ElevenLabs agent test (`agent_duplex.rs:651`). Phonic has `bounded` (20 s) and Deepslate `within_deadline` (25 s). A close that the reading server does not end would run to the runner's 60 s. No run reached it: with both timeouts removed the close ended on `read_again`.

7. **note, the test there before, the page sentence introduced, CONFIRMED.** `quality/not-measured.md:94-95` says "by reading the tests none aims at that deadline".
   - `messages_trickling_in_after_close_stream_do_not_stretch_the_summary_deadline` (`adapters/inference-deepgram/tests/bounds.rs:71`) is named for it; the source calls the handle's deadline "the summary's one deadline" (`recogniser.rs:371`).
   - With the constant at 3 s (handle deadline 6 s) it passed in `[ 3.011s]`, as did `a_server_silent_after_close_stream_stalls_within_the_read_timeout` (`errors.rs:165`). So none fires it; the thread's own deadline ends both.

8. **note, expected by rule.** Entries of `quality/adapters.md` that name only the older close tests: `:239-243`, `:253-256`, `:325-328`, `:389-392`, `:747-750`.

9. **note, for the coordinator.** Acceptance line 2 says "the story says". The story body still reads "inferred; needs a run with each planted change". The result from runs is in `quality/not-measured.md:89-93` and `:334-337`, and I confirm it below.

## Mutants

Each cell is the new test's result; times are nextest's. "agent" and "transcription" are the two tests of a crate.

| Mutant | Phonic | Deepgram agent | Deepgram transcription | Deepslate | ElevenLabs agent | ElevenLabs speech to text |
|---|---|---|---|---|---|---|
| constant itself 3 s | killed, held 3.001279769s | killed, 3.001378905s | killed, gone 3.003053809s | killed, 3.001370823s | killed, 3.001313169s | killed, gone 3.003746305s |
| bound 500 ms | killed, 500.778527ms "before its bound" | killed, 501.756266ms | killed, 503.142892ms | killed, 500.748713ms | killed, 509.579268ms | killed, 506.143279ms |
| bound 1,900 ms | green | green | green | green | green | green |
| bound 1,500 ms | green | green | green | green | green | green |
| frame-write timeout removed | green, 1.001288239s | green, 1.001258809s | killed at own deadline, `FAIL [ 4.263s]` | green, 1.001477105s | green, 1.001327359s | killed at own deadline, `FAIL [ 4.227s]` |
| wait-for-task timeout removed | green, 1.001784973s | green, 1.001539053s | not applicable | green, 1.001861971s | green, 1.001724553s | not applicable |
| both removed | killed, `FAIL [ 4.315s]` | killed, `FAIL [ 4.502s]` | killed, `FAIL [ 4.276s]` | killed, `FAIL [ 4.413s]` | killed, `FAIL [ 4.161s]` | killed, `FAIL [ 4.289s]` |
| close not attempted | killed, 161.058µs | killed, 155.331µs | killed, gone 5.121262ms | killed, 229.093µs | killed, 234.451µs | killed, gone 5.215073ms |
| fixture reads on | killed, 8.335 s | killed, 10.757 s (other assertion) | green 1 of 4 | killed, 11.894 s (other assertion) | killed, 6.965 s | green 2 of 4 |
| fixture deaf, no flood | killed, `a close frame behind pongs never written` | killed, same | killed, same | killed, same | killed, same | killed, same |

- The greens for one timeout removed and for 1,500 and 1,900 ms are named by the page as not told apart. Row "fixture reads on" is finding 1.
- Messages at the own deadline: `the close did not end within 4s` and `the connection thread did not end within 4s of the cancel`.
- No output of any run holds `TIMEOUT`, `LEAK` or a signal: every failing test ended by itself and its process ended.

## The tests

- **Limits:** literals only (`Duration::from_secs(1)`, `from_secs(2)`, the 4 s deadlines). `CLOSE_BOUND` appears in the new tests only in doc comments.
- **Exits of the loop:** the limits are asserted only under fewer pongs than pings; every other round doubles or fails at the largest flood. No exit passes without the limits.
- **What the fixture counts:** pongs and a close frame, only after `read_again`, to the end of the stream; the test reads the count after the fixture marks the stream ended. A close frame that got through implies every pong before it got through.
- **`/proc/self/task`:** every job in `.github/workflows/` runs on `ubuntu-latest`, and `Taskfile.yml` runs these binaries only through cargo-nextest (`cargo test` runs doc tests only).
- **Ten runs each, committed tree:** 10 of 10 pass for all six. Phonic 1.297–1.442 s, Deepgram agent 1.309–1.410 s, Deepgram transcription 1.258–1.383 s, Deepslate 1.388–2.143 s, ElevenLabs agent 1.203–1.482 s, ElevenLabs speech to text 1.197–1.425 s.
- **Suites, all features, committed tree:**
  - `inference-phonic`: `Summary [ 4.772s] 34 tests run: 34 passed, 2 skipped`, exit 0.
  - `inference-deepgram`: `Summary [ 30.816s] 131 tests run: 131 passed, 3 skipped`, exit 0. The pool ran it one test at a time (the test times sum to 30.794 s); the two new tests are 2.95 s of it.
  - `inference-deepslate`: `Summary [ 2.746s] 72 tests run: 72 passed, 1 skipped`, exit 0.
  - `inference-elevenlabs`: `Summary [ 7.226s] 169 tests run: 167 passed, 2 failed, 5 skipped`, exit 100. The two are the known ticker tests (`async_handles.rs:192` gap 175.879683ms, `:134` gap 165.799818ms).
  - `inference-docs`: `Summary [ 0.850s] 74 tests run: 74 passed, 0 skipped`, exit 0.
- **Base comparison, Phonic:** `Summary [ 1.872s] 33 tests run: 33 passed, 2 skipped` at the base files; the two result lists differ by one line, the new test.

## Pages, acceptance, nothing left behind

- **Pages:** every named test exists under its name in its file. The Cartesia and ElevenLabs synthesisers do wait for a close exchange with no deadline (`inference-cartesia/src/synthesiser.rs:362-365`, `inference-elevenlabs/src/synthesiser.rs:388`). The Deepgram handle deadline is at `recogniser.rs:377`. No anchor link to a changed heading exists in the tree. Exceptions: findings 2, 3, 7.
- **`git diff eb32eb8b2 HEAD --stat`:** 14 files, twelve under `adapters/*/tests`, plus `quality/adapters.md` and `quality/not-measured.md`. Nothing under `quality/adapters.ratchet.yaml`, `quality/records/` or `crates/inference-docs/`.
- **Acceptance 1:** held for all six at 3 s. An enlargement under 2 s passes (finding 2).
- **Acceptance 2:** held by runs for all four session bindings; the story body does not say it yet (finding 9).
- **Acceptance 3:** no new test over 5 s in 60 passing runs (largest 2.143 s); each ends its own wait at 4.16–4.50 s when nothing ends the close. The 120 s of the gate's test step I did not measure.
- **Nothing left behind:** `git diff eb32eb8b2 HEAD -- ':(glob)adapters/*/src/**' ':(glob)**/Cargo.toml'` is empty; no `deaf.rs` or `dropped.rs`; no new file under any `tests/`.

## Not attacked

- `task test-run` over the whole workspace, and its 120 s.
- Another kernel, the forge's runner, the workstation.
- The tests under `cargo test` (one process).
- A dropped handle or session, the end of input, a failed open.
- A close whose wait is skipped while the task still closes.
- The sentence at `quality/not-measured.md:117-121` about what the same peer would fire elsewhere.
- `cargo fmt` and `cargo clippy`.
- The session tests against a reading fixture more than once each.

## Paths written outside the worktree

All under <scratch>:

- Scripts: `plant-session.sh`, `plant-fixtures.sh`, `run.sh`, `matrix.sh`, `all-matrices.sh`, `extra.sh`, `literal.sh`, `restore.sh`, `ten.sh`, `suites.sh`, `base-phonic.sh`, `final.sh`
- `backup/` (HEAD's content of the 13 planted files)
- `patch/` (the planted diffs; `all-planted.patch` is all of them)
- `out/` (one file per mutant run, `all-matrices.log`, `extra.log`, `final.log`, `restore-1.txt`)
- `ten/` (four files), `suites/` (five files), `base/` (the base run, the two lists, HEAD's copies of the two Phonic files)

```findings
- file: adapters/inference-deepgram/tests/cancel.rs
  line: 174
  category: mutant
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: with the fixture planted to read on, the two transcription tests pass in 3 of 8 runs at the 576,000 flood because fewer pongs than pings is also true of a peer that is only slow; reached only by the plant or by buffers of 37.7 MB
- file: quality/adapters.md
  line: 70
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the page says the six tests fail with the bound enlarged, and all six pass with a bound of 1,500 ms and of 1,900 ms
- file: adapters/inference-phonic/tests/duplex.rs
  line: 510
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the lower limit passes a constant of 999 ms (held 1.000308908 s), where the page says the limits pass a bound from 1 s
- file: adapters/inference-deepgram/tests/cancel.rs
  line: 78
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the 15 bytes the transcription tests match are also the name of the crate's clock thread (wake.rs:66, and wake.rs:63 and synthesiser.rs:358 in ElevenLabs), which neither test starts
- file: adapters/inference-deepslate/tests/opal_duplex.rs
  line: 1083
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the comment says 36,000 pongs are more than the buffers hold, and on the build node they all arrived in 5 of 10 runs, so the test took a second round (up to 2.143 s)
- file: adapters/inference-deepgram/tests/agent_duplex.rs
  line: 720
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: closer.join() has no bound of its own in the Deepgram agent and ElevenLabs agent tests (agent_duplex.rs:651), only the runner's 60 s; no run reached it
- file: quality/not-measured.md
  line: 94
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the page says no Deepgram test aims at the handle deadline, while bounds.rs:71 is named for the summary deadline; a run with the constant at 3 s shows it is ended by the thread's own deadline, so none fires it
- file: quality/adapters.md
  line: 239
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: the close entries at 239, 253, 325, 389 and 747 name only the older close tests, as expected until a check run on main
- file: quality/not-measured.md
  line: 89
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: acceptance line 2 asks that the story say what each removal alone does; the page and the runs say it (green for each alone in all four), the story body still says inferred
```
