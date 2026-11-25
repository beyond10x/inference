---
format: aep.planning-md/3
id: review-result:synthesiser-drop-inside-runtime-adversary-2
kind: review-result
status: active
title: Adversary pass 2 on the synthesiser drop
relations:
- reviews: story:synthesiser-drop-inside-runtime
revision: 1
---
Adversary, pass 2 (the last), on story:synthesiser-drop-inside-runtime, 2026-11-25, as the adversary returned it; `<scratch>` is the pass's scratch directory outside the repository, and the name of the build node is replaced by `<build node>`.

unit: story:synthesiser-drop-inside-runtime
verdict: red
cases: executed 241→244, red 1
origin: introduced 6, pre-existing 0, undecided 0
wrote-outside-worktree: <scratch>/logs/ (24 files), <scratch>/mutant/ (source copy), <scratch>/mutants/ (8 files), <scratch>/probe/absorb.py, <scratch>/run.sh, <scratch>/mutant.sh, <scratch>/step2.sh, <scratch>/step3.sh, <scratch>/suite.sh
needs-coordinator: yes

Covers head `69f0e6d88` on base `97251ed53`, plus my one untracked test file. `<scratch>` is the directory the brief assigned. `241` is the implementor's reported count; I ran no suite before my cases existed.

The drop itself held up: I found no defect in `src` that a Tokio caller meets. The findings are four gaps in the tests of the correction, one doc sentence wider than the code, and the fixed flood.

Needs the coordinator for three decisions:
- **My file:** cases 1 and 3 are green at head and kill three mutants the committed suite lets through. Case 2 is red and stays red until row 5 is ruled, so the unit cannot take the file as it is.
- **Cartesia:** with the grace removed, its drop cases passed three of three runs. It needs a copy of case 1, or the gap is accepted.
- **Row 5:** whether the docs say "a Tokio runtime", or the drop changes.

## 1. Diff

`git --no-pager diff --stat`: empty. `git status --short`:

```
?? adapters/inference-elevenlabs/tests/adversary2_w2_drop_any_host.rs
```

- The one path is a new test file; no `src` file and no existing test was edited.
- Nothing was committed, stashed or branched.
- Mutants were planted in a scratch copy of the head (`git archive`), never in the worktree. The brief allowed the tree; my procedure does not.
- The lease `w2-u2-adversary-drop-2` is released.
- The file passes `cargo fmt -p inference-elevenlabs --check` and `cargo clippy -p inference-elevenlabs --all-targets --all-features -- -D warnings`.

## 2. Cases added

`adapters/inference-elevenlabs/tests/adversary2_w2_drop_any_host.rs`, three cases, none over 5 s (2.1 to 2.9 s, 1.4 to 1.8 s, 0.4 to 0.5 s).

The fixture is the committed deaf server with one change: when told, it reads the socket again and counts frames. Fewer pongs than pings means the rest never left the client, so the close behind them was backed up. A flood whose pongs all arrived is doubled and the session runs again. The flood starts at 9,000 on purpose, so the doubling runs on the build node:

```
net.ipv4.tcp_wmem: Ok("4096\t20480\t4194304")
a flood of 9000 pings: the server then read Drained { pongs: 9000, others: [1, 8], close_sockets: 1, late_pongs: 0, unframed: 0 }
a flood of 18000 pings: the server then read Drained { pongs: 18000, others: [1, 8], close_sockets: 1, late_pongs: 0, unframed: 0 }
a flood of 36000 pings: the server then read Drained { pongs: 32269, others: [], close_sockets: 0, late_pongs: 0, unframed: 119 }
```

**Case 1, `a_drop_whose_close_cannot_be_written_holds_the_runtime_for_the_grace_and_no_longer`: green at head.**
- Asserts the ticker's bound, `CLOSE_GRACE <= held < CLOSE_GRACE + 15 ms`, the close still waiting 300 ms later, no close arriving, and the same descriptor count as after a session whose close was written.
- At head: held 5.147 to 5.169 ms, gap 11.5 to 14.4 ms, 5 descriptors after every session.

**Case 2, `a_drop_inside_an_executor_that_is_not_tokio_does_not_hold_its_thread`: red at head.**
- Drops the idle handle inside a hand-written executor (poll, park) on a backed-up connection and asserts the hold is under 50 ms.
- First run, alone, before anything else ran (`<scratch>/logs/head-any-host-1.log`):

```
thread 'a_drop_inside_an_executor_that_is_not_tokio_does_not_hold_its_thread' (1426640) panicked at adapters/inference-elevenlabs/tests/adversary2_w2_drop_any_host.rs:345:5:
the drop inside an async executor held the executor's thread 1.000151076s, where `src/lib.rs` says a drop inside an async runtime waits at most 5ms
```

- The assertion is now at line 440. Red in 4 of 4 head runs.

**Case 3, `a_close_handed_off_is_written_once_when_the_server_reads_again`: green at head.**
- The server reads again 100 ms after a drop that handed its close off.
- Asserts every pong arrives, then exactly one `close_socket` and one close frame and nothing else, and the own thread ends before `CLOSE_BOUND`.
- At head: `pongs: 36000, others: [1, 8], close_sockets: 1, late_pongs: 0, unframed: 0`; held 5.10 and 5.21 ms; own thread ended 118.7 and 143.8 ms after the drop.

**Mutants, each in the scratch copy (`<scratch>/mutants/*.diff`):**

| Mutant | Committed cases red | My cases red |
|---|---|---|
| ElevenLabs, no grace (`wait` = zero inside a runtime), 3 runs | run 1: both process-exit cases (1 and 2 of 10 lost); runs 2 and 3: none | case 1 in 3 of 3 (`held 60.15µs`, 54.3 µs, 43.8 µs) |
| Cartesia, no grace, 3 runs of `adversary_w2_drop_process_exit`, `adversary_w2_drop_backed_up`, `cancel` | none in 3 of 3 (`8 tests run: 8 passed` each) | no Cartesia case written |
| ElevenLabs, `CLOSE_GRACE * 6`, 2 runs | none (hold 30.1 ms, gap 30.2 to 33.4 ms) | case 1 in 2 of 2 (`held its thread 30.180919ms`) |
| ElevenLabs, handed-off close aborted while the thread lingers `CLOSE_BOUND`, 1 run | none (14 passed) | case 3 (`left: (32447, 0) right: (36000, 0)`, no close arrived) |

Not planted: "the wait kept and the close not written inside the grace". By reading, `closed` at `adversary_w2_drop_process_exit.rs:123` and `async_handles.rs:179` go red there.

## 3. Suite run

Run after part 2, in the worktree at head:

```
CARGO_INCREMENTAL=0 cargo nextest run -p inference-elevenlabs -p inference-cartesia --all-features --no-fail-fast --failure-output final --success-output final
```

Output (`<scratch>/logs/suite-all-1.log`):

```
slot: synced to <build node> in 0.7 s, 126490 bytes sent, 4 sibling mirrors as base, at 69f0e6d88 + 1 changed file
    Starting 244 tests across 51 binaries (7 tests skipped)
        FAIL [   1.402s] (170/244) inference-elevenlabs::adversary2_w2_drop_any_host a_drop_inside_an_executor_that_is_not_tokio_does_not_hold_its_thread
     Summary [   5.655s] 244 tests run: 243 passed, 1 failed, 7 skipped
    thread 'a_drop_inside_an_executor_that_is_not_tokio_does_not_hold_its_thread' (2005617) panicked at adapters/inference-elevenlabs/tests/adversary2_w2_drop_any_host.rs:440:5:
    the drop inside an async executor held the executor's thread 1.000138821s, where `src/lib.rs` says a drop inside an async runtime waits at most 5ms
error: test run failed
```

Exit 100.

The three runs the brief asked for, same command with `-E 'not binary(adversary2_w2_drop_any_host)'` (my binary excluded, so its floods do not load the ticker tests):

```
committed 1:      Summary [  11.614s] 241 tests run: 241 passed, 7 skipped EXIT=0
committed 2:      Summary [  10.358s] 241 tests run: 241 passed, 7 skipped EXIT=0
committed 3:      Summary [   8.338s] 241 tests run: 241 passed, 7 skipped EXIT=0
```

- No case was red in any of them.
- The slowest test is an existing one at 4.0 s; nothing reached the 60 s bound.
- Over the four runs: 120 children, every drop returned within the grace (130 to 681 µs), 0 handed off, no close lost.
- Committed backed-up cases: held 5.10 to 5.24 ms, gap 11.3 to 16.4 ms.

## 4. Findings

| # | file:line | verdict | origin | what was measured | what reaches it |
|---|---|---|---|---|---|
| 1 | `adapters/inference-elevenlabs/tests/adversary_w2_drop_process_exit.rs:139` | NEEDS-CHANGE | introduced | With the grace removed, both cases passed in 2 of 3 runs: all ten drops returned in microseconds and no child lost its close. The case is a race it mostly wins on the build node. | The gate: a later change that drops the grace passes it most of the time. Case 1 fails on that mutant every run. |
| 2 | `adapters/inference-cartesia/tests/adversary_w2_drop_process_exit.rs:151` | NEEDS-CHANGE | introduced | With the grace removed from the Cartesia drop, all 8 Cartesia drop cases passed in 3 of 3 runs. Nothing in the suite failed on the Cartesia grace being gone. | Same as row 1. No Cartesia copy of case 1 exists; I did not write one. |
| 3 | `adapters/inference-elevenlabs/src/synthesiser.rs:803` | NEEDS-CHANGE | introduced | A drop that waits six times `CLOSE_GRACE` passes all 13 committed drop cases in 2 of 2 runs (hold 30.1 ms, gap 30 to 33 ms). "At most `CLOSE_GRACE`" (`src/lib.rs:45`) has no test; only the 50 ms ticker bound limits the hold. | A caller who relies on the documented 5 ms. Case 1 fails at `:388`. Cartesia `src/synthesiser.rs:662` is the same line; not run. |
| 4 | `adapters/inference-elevenlabs/src/synthesiser.rs:812` | NEEDS-CHANGE | introduced | No committed case meets a handed-off close that is then written; they meet it only where it is never written. A hand-off that aborts the close and lingers `CLOSE_BOUND` passes all 14 committed drop cases. | The doc sentence "runs to its end on the handle's own thread" (`src/lib.rs:46`). Case 3 fails on the mutant. Cartesia `:671` is the same line; not run. |
| 5 | `adapters/inference-elevenlabs/src/lib.rs:44` | INFEASIBLE | introduced | The docs say "Outside an async runtime" and "Inside one"; the drop asks Tokio (`src/synthesiser.rs:802`). Inside an executor that is not Tokio, on a backed-up connection, the drop held the thread 1.0001 to 1.0002 s in 4 of 4 runs. The block is as at base; the sentence is new. | Nothing found. I built both the executor and the backed-up connection. The repository's own speech node (`crates/inference-pipeline/src/nodes/mod.rs:253`) runs one future per thread on such an executor, where the wait is harmless and sends the close. Same words at Cartesia `src/lib.rs:30`, `src/synthesiser.rs:661`. |
| 6 | `adapters/inference-elevenlabs/tests/adversary_w2_drop_backed_up.rs:49` | INFEASIBLE | introduced | The fixed 36,000 pings sit 10 % above what the build node's buffers took (32,269 to 32,447 pongs over 9 floods). Floods of 9,000 and 18,000 did not back the close up. The limit is the kernel's largest send buffer, which the case does not control. | Nothing found: the build node and this workstation have the 4 MiB default, and the forge runs `ubuntu-latest`. A host with a raised `net.ipv4.tcp_wmem` maximum fails both backed-up cases on "the close still waits". Same constant at Cartesia `:48`. |

Fixes I name and did not apply:
- **Rows 1 to 3:** take case 1, and a Cartesia copy of it.
- **Row 4:** take case 3.
- **Row 5:** say "a Tokio runtime" in the four doc places, then drop case 2 or turn it to assert the documented wait.
- **Row 6:** yes, there is a way without the kernel setting, and case 1 is it: the fixture counts the pongs it finally reads and the case doubles the flood until some are missing. Past 576,000 pings it fails with a message that names the host. Started at 36,000 it costs what the committed case costs on a default host.

On the brief's question whether the missing bound is a defect a caller meets: not at head, where the hold is 5.1 to 5.2 ms. It is a gap in the suite (rows 1 to 3).

## 5. Attacked and not broken

- **`held < CLOSE_GRACE` is a sound sign:** a hand-off returns only after an instant taken inside the drop plus the grace, so its hold is never under the grace. By reading; 120 of 120 children were within it.
- **Hand-off, close written once:** case 3, one `close_socket`, one close frame, nothing after, own thread gone 119 to 144 ms after the drop.
- **Hand-off, threads and descriptors:** case 1, 0 own threads and the same 5 descriptors after a handed-off drop. One hand-off per case; forty floods in a row would be load on purpose, so not run.
- **Who polls the close:** the drop polls only the task's join handle; the close future runs on the handle's own runtime alone. A close that panics makes the join ready and the drop returns. By reading, no case.
- **Inside a Tokio runtime:** `spawn_blocking`, `block_in_place`, an entered thread, a panic unwinding and the outer runtime shutting down all count as inside; the committed `adversary_w2_drop_where.rs` cases were green in every run.
- **`ended_in_order` removed:** `async_handles.rs:179` compares the last two opcodes with `[TEXT, CLOSE]` and `cancel.rs` compares all of them, so a missing close, a doubled close and a close before the last text each fail. By reading, not planted.
- **Docs:** both crates say a process that ends during a handed-off close loses the close frame, and ElevenLabs adds `close_socket` alone or neither. No other document in the repository speaks of the drop.
- **Three suite runs:** green, see part 3.

Refused actions:
- A hook refused one Bash command of mine that held a heredoc: `b10x guard (heredoc_stdin): a heredoc into a stdin reader has hung a session for 22 h: write the script with the Write tool and run it by path`. It was an empty placeholder; I did not retry it.
- I did not write `<scratch>/report.md`: my harness rules forbid report files for a sub-agent. This message is the report.

## 6. Paths written outside the worktree

- `<scratch>/logs/`, 24 files:
  - `head-any-host-1.log` to `head-any-host-3.log`, `head-fmt.log`, `head-fmt-2.log`, `head-clippy.log`, `head-clippy-2.log`
  - `mutant-no-grace-1.log` to `-3.log`, `mutant-six-graces-1.log`, `-2.log`, `mutant-abort-and-linger-1.log`, `mutant-no-grace-cartesia-1.log` to `-3.log`
  - `suite-committed-1.log` to `-3.log`, `suite-all-1.log`, `suite.txt`
  - `probe-this-host.log`, `step2.done`, `step3.done`
- `<scratch>/mutant/`: a `git archive` copy of the head without `.engineering`, `website` and `ux`, plus my test file. Its two `src` trees are back to the head's sources. Its build output is on the build node.
- `<scratch>/mutants/`: four diffs and the four mutated `synthesiser.rs` files.
- `<scratch>/probe/absorb.py`: a scratch probe, run once on this workstation, not for the repository. A plain writer to a peer that reads nothing stalled at 2.7 MB here.
- `<scratch>/run.sh`, `<scratch>/mutant.sh`, `<scratch>/step2.sh`, `<scratch>/step3.sh`, `<scratch>/suite.sh`: run scripts.

## 7. Findings block

```findings
- file: adapters/inference-elevenlabs/tests/adversary_w2_drop_process_exit.rs
  line: 139
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: with the grace wait removed both process-exit cases passed in 2 of 3 runs, so the only committed test of the grace is a race it mostly wins
- file: adapters/inference-cartesia/tests/adversary_w2_drop_process_exit.rs
  line: 151
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: with the grace wait removed from the Cartesia drop all 8 Cartesia drop cases passed in 3 of 3 runs
- file: adapters/inference-elevenlabs/src/synthesiser.rs
  line: 803
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a drop that waits six times CLOSE_GRACE inside a runtime passes all 13 committed drop cases, so the documented at-most bound has no test
- file: adapters/inference-elevenlabs/src/synthesiser.rs
  line: 812
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a hand-off that aborts the close and lingers CLOSE_BOUND passes all 14 committed drop cases, because no case meets a handed-off close that is then written
- file: adapters/inference-elevenlabs/src/lib.rs
  line: 44
  category: contract-drift
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the docs say a drop inside an async runtime waits at most CLOSE_GRACE but the drop asks Tokio only, and inside another executor on a backed-up connection it held the thread 1.0 s, in a state the case constructs
- file: adapters/inference-elevenlabs/tests/adversary_w2_drop_backed_up.rs
  line: 49
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the fixed flood of 36,000 pings is 10 % above what the build node's socket buffers took and fails the case on a host with a larger tcp_wmem maximum, which no host I could read has
```
