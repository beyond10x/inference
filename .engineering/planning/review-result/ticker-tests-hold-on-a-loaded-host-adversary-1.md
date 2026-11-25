---
format: aep.planning-md/3
id: review-result:ticker-tests-hold-on-a-loaded-host-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the ticker tests
relations:
- reviews: story:ticker-tests-hold-on-a-loaded-host
revision: 1
---
Adversary pass 1 on story:ticker-tests-hold-on-a-loaded-host, 2026-11-24, as the adversary returned it; `<scratch>` is the pass's scratch directory outside the repository.

```
unit: story:ticker-tests-hold-on-a-loaded-host, head c825c01bcc8545df53acbddb8495f9701dcdbae9 plus one untracked test file
verdict: red (one finding, INFEASIBLE, severity warning; no blocker; the acceptance command passed 15 of 15 runs)
cases: executed 163→170, red 1 (one case, red in 1 of 17 runs on the node as it was and 1 of 1 confined to one processor; green in the last run)
origin: introduced 1, pre-existing 0, undecided 0
wrote-outside-worktree: 56 files, all under <scratch>/ (part 6)
needs-coordinator: yes (does the acceptance sentence cover a thread held while it stays ready to run; and the case file left behind must not be committed as it is)
```

## 1. What I touched

`git --no-pager diff --stat` is empty: no tracked file changed, nothing under `src`, nothing under `.engineering/`. `git status --short`:

```
?? adapters/inference-elevenlabs/tests/adversary_w2_ticker_blocks.rs
```

- I made no `src` edit. The brief allowed a temporary mutant in `src`; the adversary procedure forbids it, so every block was planted as a session future on a copy of the helper instead.
- A second file, `adversary_w2_ticker_probe.rs` (measurements only), was in `tests/` during the runs and is removed; its source is `<scratch>/adversary_w2_ticker_probe.rs.txt`.
- Lease `w2-u2-adversary-1` is released.

## 2. The cases I added

File: `adapters/inference-elevenlabs/tests/adversary_w2_ticker_blocks.rs`, 7 tests, about 0.7 s each, clippy and rustfmt clean.

- It holds a verbatim copy of `grow_descriptor_table`, `ReadyWait` and `with_ticker`, because they are private to the `async_handles` target.
- `the_copy_is_the_helper_of_async_handles` compares the copy with `async_handles.rs` by text and fails when they differ.
- Each of the other six holds the ticker's thread once for 200 ms between two awaits and asserts `largest >= GAP_BOUND`, the opposite of what the two tests assert. The four sleeping forms also assert `largest >= 200 ms` ("counted in full", the test file's own sentence).

| case | form | now | all runs |
|---|---|---|---|
| `a_thread_asleep_for_200_ms_fails_the_tests` | `thread::sleep` | green | gap 202 to 211 ms |
| `a_thread_parked_on_a_second_runtime_for_200_ms_fails_the_tests` | park until a second runtime's task ends (the adapter's `wake::block_on` shape) | green | 205 to 207 ms |
| `a_thread_waiting_200_ms_for_a_lock_fails_the_tests` | mutex held by another thread | green | 204 to 209 ms |
| `a_thread_waiting_200_ms_in_a_blocking_read_fails_the_tests` | blocking socket read | green | 204 to 211 ms |
| `a_thread_computing_for_200_ms_fails_the_tests` | busy loop | green | 202 to 203 ms; 182 ms confined to one processor |
| `a_thread_yielding_in_a_loop_for_200_ms_fails_the_tests` | loop of `yield_now` | green in the last run | red in 1 of 17 runs on the node as it was, red in 1 of 1 confined to one processor |

Order of runs, as it happened:

- **First run of anything** (`<scratch>/cases-1.log`): 6 passed, and my park case hung and was killed at 60 s. That was my own bug: I built the sleep on the wrong runtime. I fixed the case; it is not a finding.
- **Second run** (`<scratch>/cases-2.log`): the yield case went red. Verbatim:

```
        FAIL [   0.694s] (2/7) inference-elevenlabs::adversary_w2_ticker_blocks a_thread_yielding_in_a_loop_for_200_ms_fails_the_tests
  stdout ───

    running 1 test
    a loop of yields: the thread was held 200.445375ms, the ticker's largest gap is 11.412421ms
    test a_thread_yielding_in_a_loop_for_200_ms_fails_the_tests ... FAILED

    test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 6 filtered out; finished in 0.69s

  stderr ───

    thread 'a_thread_yielding_in_a_loop_for_200_ms_fails_the_tests' (2565079) panicked at adapters/inference-elevenlabs/tests/adversary_w2_ticker_blocks.rs:173:5:
    a loop of yields held the runtime's thread 200.445375ms and the ticker's largest gap is 11.412421ms: under the bound, the two tests pass
```

- **Alone, six runs** (`<scratch>/yield.txt`): green 6 of 6, gap 202 to 203 ms, load 74 to 85 on 64 processors.
- **Confined to one processor** (`slot taskset -c 21 cargo nextest run ... a_thread_yielding a_thread_computing`, `<scratch>/one-processor-1.log`): the yield case red, held 217.9 ms, gap 11.2 ms; the busy loop held 200.3 ms, gap 181.9 ms, so it still fails the tests.

After that I added a load line to the case's output and formatted the file; the assertion is now at line 180.

## 3. The suite runs, after the cases existed

Acceptance, `cargo nextest run -p inference-elevenlabs --all-features --test async_handles`, ten runs (`<scratch>/accept.txt`): every line `exit 0` and `3 tests run: 3 passed, 0 skipped`, 1.48 to 1.87 s.

Five more with my file added, `--test async_handles --test adversary_w2_ticker_blocks`, at a 1-minute load of 129 to 162 (`<scratch>/accept-loaded.txt`): every line `exit 0`, `10 tests run: 10 passed, 0 skipped`.

Package lane, `cargo nextest run -p inference-elevenlabs --all-features`, three runs (`<scratch>/package.txt`):

```
run 1 exit 0:     Summary [   5.730s] 170 tests run: 170 passed, 5 skipped
run 2 exit 0:     Summary [   5.788s] 170 tests run: 170 passed, 5 skipped
run 3 exit 0:     Summary [   5.599s] 170 tests run: 170 passed, 5 skipped
```

- `executed <before>` is the implementor's package run: 163 run, 5 skipped. After: 170, the 7 added.
- `cargo test -p inference-elevenlabs --all-features --test async_handles` (one process, three tests as threads), five runs (`<scratch>/cargotest.txt`): 5 of 5 `3 passed; 0 failed`.

The suite did not go red. The two ticker tests held on the node in every run I made.

## 4. Finding

Covers head c825c01bc.

| # | file:line | verdict | origin | finding |
|---|---|---|---|---|
| 1 | `adapters/inference-elevenlabs/tests/async_handles.rs:124` | INFEASIBLE | introduced | A runtime thread held 200 ms in a loop of `std::thread::yield_now()` is counted as an 11 ms gap when another thread is ready on the same processor, so `largest < GAP_BOUND` passes. |

- **What was measured:** `adversary_w2_ticker_blocks.rs:180`, exit 100, in `<scratch>/cases-2.log` and `<scratch>/one-processor-1.log`. The thread stays ready to run the whole time, so the kernel counts the block as run-queue wait and `ReadyWait` takes it out.
- **Origin:** at the base the gap was the clock's, 200 ms, and was caught.
- **What reaches it:** nothing at head. No file under `adapters/inference-elevenlabs/src` waits by yielding or spinning (`grep` for `yield_now` and `spin_loop`: none). It needs a future change that waits that way, and a shared processor: 1 of 17 runs on the node as it was, at loads from 52 to 162.
- **Why it is small:** a computing block under the same confinement lost only 18 of 200 ms and still fails the tests.
- **Fix, named and not applied:** one sentence in the module doc at `async_handles.rs:16-19` saying that a thread which stays ready to run (a yield loop, a spin) is taken out of the gap when its processor is shared. The kernel's second schedstat field cannot tell that wait from a late host.

## 5. Attacked and not broken

- **The cause:** reproduced independently. Table not grown: gap 108.9 ms (syntheses) and 77.2 ms (recognitions) with 0 ms of run-queue wait in it. Table grown: 11.7 and 11.8 ms (`<scratch>/probe-1.log`).
- **Blocking forms:** sleep, park on a second runtime's task, lock and socket read are counted in full, 202 to 211 ms, in every run, including at load 129 to 162. A blocking DNS lookup or connect was not run; a blocking read stands in for them.
- **`schedstat` field 2:** it is the thread's run-queue wait in nanoseconds; the file was read and held three fields on the node. A missing or unreadable file gives zero, so nothing is taken out and the test is stricter, not skipped.
- **What `ReadyWait` does in the real tests:** 22 runs with the table grown (`<scratch>/probe-grown-*.log`, `<scratch>/probe-loaded-*.log`).
  - Load 52 to 64: at most 68 µs taken out of any gap; clock gap 11.5 to 12.2 ms.
  - Load 98 to 110: up to 20.7 ms taken out of one gap; largest clock gap 31.1 ms, counted 11.7 ms.
  - No run reached 50 ms by the clock, so `ReadyWait` changed no result in 22 of 22. It still has no red run of its own, but the clock gap came within 19 ms of the bound.
- **Sum of small blocks:** 20 sleeps of 35 ms, 60 ms apart, held the thread 702 ms of 1.50 s; largest gap 44.4 ms, which passes. Run-queue wait in it was 0, so the base helper gives the same number: this is the largest-gap design, not the subtraction.
- **A block spanning a tick:** cannot be split. The ticker runs on the blocked thread, and blocks in the same turn add up into one gap.
- **Smallest block caught:** one sleep in a later poll of one session gave 45 ms → 48.4 ms (passes), 48 ms → 51.4 ms (fails), 60 ms → 63.4 ms, 100 ms → 103.6 ms.
  - By the helper's arithmetic the gap is the block plus 0 to about 11 ms: always caught from 50 ms, never under about 39 ms, between the two by where in the tick it lands.
  - Measured with a plain future on the helper, not with the adapter mutated.
- **Descriptors:** the twenty sessions hold 135 (136 counted, one of them my reading). A session that opens 599 in a later poll, past the grown 512, took 92.6 ms and was counted 96.2 ms, so that wait is not hidden; 199 took 55 µs.
- **Descriptor limit:** under a limit of 200 the grown-table run passes (gap 12.2 ms). Under 160 the sessions themselves fail with "Connection refused"; the growth holds nothing by then, so that failure is not its doing.
- **The agent test:** its sessions open before the ticker; the helper change gives it the same subtraction and nothing else. It passed 23 of 23 runs.
- **`cargo test` in one process:** three tests growing one table at once, 5 of 5 green.

## 6. Paths written outside the worktree

All under `<scratch>/` (the directory `wave2/u2/adversary-ticker-1` named in the brief):

- Scripts: `run.sh`, `repeat.sh`, `cargotest.sh`.
- Sources: `adversary_w2_ticker_blocks.rs.txt`, `adversary_w2_ticker_probe.rs.txt`.
- Case runs: `cases-1.log`, `cases-2.log`, `cases-final.log`, `yield.txt`, `yield-1.log` to `yield-6.log`, `one-processor-1.log`.
- Suite runs: `accept.txt`, `accept-1.log` to `accept-10.log`, `accept-loaded.txt`, `accept-loaded-1.log` to `accept-loaded-5.log`, `package.txt`, `package-1.log` to `package-3.log`, `cargotest.txt`, `cargotest-1.log` to `cargotest-5.log`.
- Measurements: `probe-1.log`, `probe-grown.txt`, `probe-grown-1.log` to `probe-grown-5.log`, `probe-loaded.txt`, `probe-loaded-1.log` to `probe-loaded-5.log`, `clippy.log`.

One stray file, `probe.tmp`, was written one level up in the unit's scratch directory and removed in the next command. Builds went to the worktree's own `target/` through the pool.

For the coordinator: the yield case is red only when its processor is shared, so `adversary_w2_ticker_blocks.rs` would be a flaky test if committed as it is. The six other cases are green controls.

```findings
- file: adapters/inference-elevenlabs/tests/async_handles.rs
  line: 124
  category: acceptance
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: a runtime thread held 200 ms in a loop of yield_now is counted as an 11 ms gap when another thread is ready on its processor, so the bound passes; no code at head waits that way, and it was red in 1 of 17 runs on the node as it was and 1 of 1 confined to one processor
```

