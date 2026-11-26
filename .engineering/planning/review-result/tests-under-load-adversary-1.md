---
format: aep.planning-md/3
id: review-result:tests-under-load-adversary-1
kind: review-result
status: active
title: Adversary pass over the tests under load (2026-11-26)
relations:
- reviews: story:deepgram-keep-alive-test-flake
- reviews: story:tests-that-fail-on-a-loaded-machine
- reviews: story:drop-tests-own-limit-and-stated-bound
revision: 1
---
unit: U3 tests under load, `wave/u3-tests-under-load` at head `6ccf99849` (three story commits on `30f7fa6ef`). Worktree: <worktree>
verdict: CONFIRMED. 5 findings: blocker 0, warning 3, note 2.
cases: executed 41→45, red 1 on the unplanted tree. The other 3 added cases go red only with their mutant planted.
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory, <scratch>/wave-1126/u3-adversary/
needs-coordinator: none

## 1. Diff

`git --no-pager diff --stat` with my cases in (saved as `added-cases.patch`; `git apply --check` passes on `6ccf99849`):
```
 adapters/inference-aws/tests/nova_sonic_duplex.rs  | 31 +++++++++
 .../tests/adversary_w2_drop_backed_up.rs           | 76 ++++++++++++++++++++++  (cartesia)
 adapters/inference-deepgram/tests/audio.rs         | 31 +++++++++
```
Only test files. `git status --short` is empty now.

Charter note: the brief allowed planting defects in library and test code if restored. The adversary procedure says to mutate copies only. I followed the brief. Every plant went through `u3-adversary/plant.sh`: copy, sed, run, copy back, `cmp`. Each one printed `restored <file>`.
- Library files planted: `deepgram/src/recogniser.rs:800`, `cartesia/src/connection.rs:146`, `cartesia/src/synthesiser.rs:480,486,499,663,664,672`, `elevenlabs/src/connection.rs:150`, `elevenlabs/src/synthesiser.rs:803,812`, `aws/src/duplex.rs:984`.
- Test lines put back to their base form for single runs: `nova_sonic_duplex.rs:131`, `cartesia adversary_w2_drop_backed_up.rs:333`.

## 2. Cases added (in `added-cases.patch`)

- **`inference-aws::nova_sonic_duplex adversary_until_fails_with_its_message_on_a_slow_stream_that_never_brings_the_event`**
  - Setup: replay pace 150 ms, 1000 audio frames, `until(|_| false)` on a thread, 15 s watchdog.
  - Red now, run alone, first run: `FAIL [ 15.033s] ... until neither found the event nor failed within 15.000095136s of a stream with one event every 150 ms`, `left: Err(Timeout)`.
  - With `until` in its base form (line 131 `idle = 0;` removed): `PASS [ 3.948s]`.
- **`inference-deepgram::audio adversary_the_keep_alive_follows_the_configured_interval`**
  - Asserts two KeepAlives arrive within 2 s at a 100 ms interval.
  - Green on the head. Red with `recogniser.rs:800` planted as `keepalive_interval * 20`: `FAIL [ 4.018s] ... two KeepAlives at an interval of 100 ms took 4.006674388s`.
  - In the same run the existing `a_keep_alive_is_sent_after_silence` passed in 4.016 s.
- **`inference-cartesia::adversary_w2_drop_backed_up adversary_outside_a_runtime_the_drop_waits_the_stated_bound_and_100_ms_at_most`**
  - Asserts the hold minus ready time is at most 1 s + 100 ms.
  - Green on the head. Red with every application of the bound at 1.4 s: `held 1.400286368s, 112.103µs of it ready to run`. The rest of the crate stayed green: `49 tests run: 48 passed, 1 failed`.
- **`inference-cartesia::adversary_w2_drop_backed_up adversary_the_close_of_a_failed_synthesis_waits_the_stated_bound_and_200_ms_at_most`**
  - Asserts the stall case ends at most 1 s (stall) + 1 s (bound) + 200 ms after the chunk's audio.
  - Green on the head. Red with only `connection.rs:146` at 1.4 s: `the iterator ended 2.403168614s after the chunk's audio`. The rest of the crate stayed green: `49 tests run: 48 passed, 1 failed`.
- One probe case was not kept because it could not be made red: `u3-adversary/probe-yield-case.rs`. It is the source of finding 4.

## 3. Suite run, after the cases existed (`u3-adversary/suite.sh`)

| File | With my cases | Without them (`-E 'not test(/^adversary_/)'`) |
|---|---|---|
| cartesia `adversary_w2_drop_backed_up` | exit 0, 7 passed | 5 passed |
| elevenlabs `adversary_w2_drop_backed_up` | exit 0, 5 passed | 5 passed |
| aws `nova_sonic_duplex` | exit 100, 24 passed, 1 failed (the `until` case, 15.019 s) | 24 passed |
| deepgram `audio` | exit 0, 8 passed | 7 passed |

Determinism: each changed file run 10 times alone through the pool on the clean head. cartesia, elevenlabs, aws and deepgram each passed 10 of 10.

## 4. Findings

1. **The Deepgram keep-alive test no longer checks the interval it configures** (warning, CONFIRMED, introduced)
   - Measured: `adapters/inference-deepgram/tests/audio.rs:117`. The `KEEP_ALIVE_LIMIT` of 5 s also lets the test pass with any interval up to 2.5 s, 25 times the configured 100 ms. With `* 20` planted at `recogniser.rs:800` the test passed in 4.016 s. The base test (600 ms) would have failed.
   - What reaches it: a regression in how `ListenConfig::keepalive_interval` is used. No other listen test fires that setting; the agent tests cover the agent endpoint.
   - Fix: add an upper bound of a few intervals on the time to the second KeepAlive. The added case uses 2 s.
2. **Cartesia's drop tests pass with the close bound at 1.4 s everywhere** (warning, CONFIRMED, introduced)
   - Measured: `adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs:274` allows `STATED + 500ms` on the whole hold. The whole crate passed with 1.4 s at all applications. The constant pin in `errors.rs` stays green because the constant did not change.
   - ElevenLabs has the same 500 ms at `:275`, but its executor case allows 100 ms for the same wait (red at 1.4 s, `held 1.400209935s`).
   - What reaches it: a change at an application of the bound.
   - Fix: take `ReadyWait` out of the hold and allow 100 ms, as the added case does.
3. **The new stall case, the only test that reaches the `Connection::close` bound alone, passes with that bound anywhere from 0.6 s to 1.4 s** (warning, CONFIRMED, introduced)
   - Measured: `:491` allows 500 ms over 2 s; the lower limit is `STATED / 2`.
     - At 1.4 s on `connection.rs:146` only, every existing test passes; drops stay bounded by their own 1 s wait.
     - At 0.6 s it passed too; there the outside-runtime and does-not-block tests catch it.
   - Fix: allow 200 ms on the upper side and about 100 ms on the lower side.
4. **Taking ready time out of the hold hides a drop that waits in a loop of yields** (note, CONFIRMED, introduced)
   - Measured at `:333` (ElevenLabs `:332` is the same code). Plant: a 20 ms yield loop in the drop, 4 times `CLOSE_GRACE`.
     - Existing test passes: held 20.12 ms with 1.88 ms ready; a second run held 20.18 ms with 5.75 ms ready.
     - The base assertion fails: `held its thread 20.125312ms`.
     - Pinned to one processor beside two spinning threads: held 26.19 ms, 26.06 ms of it ready. The thread had 208 µs of processor time and 3 timeslices, so neither schedstat field can catch it, and no red case could be built.
   - What reaches it: nothing found. The drop parks today (`wake.rs` `park_timeout`).
   - Fix: the `HOLD_SLACK` doc says "The time taken out is the host's". Name the yield and spin limit there and in `quality/not-measured.md`, and consider a 5 ms `HOLD_SLACK`.
5. **`until` can run past the runner's 60 s without its own failure message** (note, INFEASIBLE, introduced)
   - Measured: `adapters/inference-aws/tests/nova_sonic_duplex.rs:131`. Each event resets the idle count, so a stream with gaps between 100 ms and 3 s ends only at its 500th event. At 150 ms gaps that is about 75 s, which ends in a nextest TIMEOUT. The base form failed with its message in 3.9 s.
   - What reaches it: nothing found. No replay in the file sends more than about 60 events (`audio_frames` is at most 20).
   - Fix: add an overall deadline to `until`, for example 20 s from its start.

## 5. Attacked and not broken

- ElevenLabs with the bound at 1.4 s in its synthesiser and connection: the executor case went red.
- Bound at 600 ms in both crates: in Cartesia the outside-runtime and does-not-block tests went red (600.6 ms and 602.4 ms); in ElevenLabs 3 of 5 tests went red.
- Interruption notice cancelling nothing, so the cancel comes from the `INTERRUPTED` end: the speech test passed. `duplex.rs` lets either event cancel, so this is not a defect.
- Failure paths: every failure path that is reachable ends with a message. Deepgram fails at 5 s, the ElevenLabs drop cases at their 10 s `LIMIT`, the Cartesia cases at their 25 s fixture limit, and Nova `until` after 3 s with no event (except finding 5).
- Not run: a count read as 0 for one event only, and a drop holding its thread on a lock.

## 6. Paths written outside the worktree

All under <scratch>/wave-1126/u3-adversary/:
- the patch: `added-cases.patch`, `diff-stat.txt`
- case sources: `aws-case.rs`, `deepgram-case.rs`, `cartesia-case-2.rs`, `cartesia-case-3.rs`, `probe-yield-case.rs`
- scripts: `plant.sh`, `suite.sh`, `ten.sh`
- outputs: `r1.out`, `r1b.out`, `r2.out` to `r10.out`, `r5b.out`, `r6a.out` to `r6c.out`, `suite-with-cases.out`, `suite-without-cases.out`, `ten.out`
- directories: `runs/` (31 files: plant logs and saved copies), `suite-with-cases/`, `suite-without-cases/`, `ten/`

## 7. Findings block

```findings
- file: adapters/inference-deepgram/tests/audio.rs
  line: 117
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: "The keep-alive test passes with the configured 100 ms interval ignored up to 2.5 s (planted x20 passed in 4.016 s), so nothing tests that ListenConfig::keepalive_interval is honoured."
- file: adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs
  line: 274
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: "With CLOSE_BOUND applied at 1.4 s at every application and the constant unchanged, the whole Cartesia crate passes because the outside-runtime limit is 1 s plus 500 ms on the whole hold."
- file: adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs
  line: 491
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: "The stall case, the only test that reaches the Connection::close bound alone, passes with that bound at 1.4 s and at 0.6 s, and with the 1.4 s plant every test of the crate passes."
- file: adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs
  line: 333
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "Taking ready time out of the hold lets a drop that yields for 4 times CLOSE_GRACE pass unloaded (base assertion red at 20.125 ms) and hides it fully under load (26.19 ms held, 26.06 ms ready)."
- file: adapters/inference-aws/tests/nova_sonic_duplex.rs
  line: 131
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: "Resetting the idle count on every event lets until run past the runner's 60 s on a stream with gaps between 100 ms and 3 s, where the base form failed with its message in 3.9 s, and no replay in the file is long enough to reach it."
```
