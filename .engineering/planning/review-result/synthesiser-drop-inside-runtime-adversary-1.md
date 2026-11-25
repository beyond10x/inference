---
format: aep.planning-md/3
id: review-result:synthesiser-drop-inside-runtime-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the synthesiser drop
relations:
- reviews: story:synthesiser-drop-inside-runtime
revision: 1
---
Adversary, pass 1, on story:synthesiser-drop-inside-runtime, 2026-11-24, as the adversary returned it; `<scratch>` is the pass's scratch directory outside the repository.

unit: story:synthesiser-drop-inside-runtime
verdict: red
cases: executed 230→242, red 4
origin: introduced 10, pre-existing 1, undecided 1
wrote-outside-worktree: <scratch>/logs/ (36 logs), <scratch>/base/ (source copy), <scratch>/m2/ (2 files), <scratch>/ten-runs.sh, <scratch>/final-runs.sh, <scratch>/closing-head.sh, <scratch>/async_handles.c825c01bc.rs, <scratch>/old-helpers.rs, <scratch>/new-helpers.rs
needs-coordinator: yes

Covers head `9fbba75ca` on base `97251ed53`, plus my five untracked test files. `<scratch>` is the directory the brief assigned. The origin line counts the 12 finding rows of part 4, not the 4 red cases (3 introduced, 1 pre-existing).

Needs the coordinator for three decisions:
- **Process exit (rows 3, 4):** either the drop keeps its close alive past a process end, or the two doc sentences are narrowed to a process that lives on.
- **Cartesia `discard` (row 5):** pre-existing and outside this story, so it needs a story of its own or a waiver.
- **My cases:** which of the five files the unit takes.

## 1. Diff

`git --no-pager diff --stat`: empty. `git status --short`:

```
?? adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs
?? adapters/inference-cartesia/tests/adversary_w2_drop_process_exit.rs
?? adapters/inference-elevenlabs/tests/adversary_w2_drop_backed_up.rs
?? adapters/inference-elevenlabs/tests/adversary_w2_drop_process_exit.rs
?? adapters/inference-elevenlabs/tests/adversary_w2_drop_where.rs
```

- Every path is a new test file; no `src` file and no existing test was edited.
- Nothing was committed, stashed or branched.
- Base and mutant were measured in a scratch copy, never in the worktree.
- The lease `w2-u2-adversary-drop-1` is released.
- The new files pass `cargo fmt --check` and `cargo clippy --all-targets --all-features -- -D warnings` for the two crates.

## 2. Cases added

The cases were written first and run alone before any suite run; the logs are in `<scratch>/logs/`. No case runs over 5 s: the slowest are the discard case at 2.9 s and forty drops at 2.4 s.

**`adapters/inference-elevenlabs/tests/adversary_w2_drop_backed_up.rs`** (no `src` change; a server that reads the segment's request and nothing after it, then sends 130,000 pings, so the client's pongs back up in front of the close)
- `dropping_an_idle_synthesis_whose_close_cannot_be_written_does_not_block_the_runtime`: the story's acceptance where the close cannot be written. It also asserts the own thread is still there 300 ms after the drop and gone once `CLOSE_BOUND` has passed.
  - Green at head: drop held 58.6 µs, gap 12.06 ms, own thread ended 1.007 s after the drop.
  - Red at base: `adversary_w2_drop_backed_up.rs:195:5: the ticker's largest gap 1.00234098s; the drop held the runtime's thread 1.000165201s` (the assertion is now at line 197).
- `..._outside_a_runtime_waits_close_bound`: green at head (1.000141 s) and at base (1.000110 s); red under the never-waits mutant (16.5 µs).

**`adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs`**
- The same two cases.
  - Drop inside a runtime: green at head (59.3 µs, gap 11.9 ms); red at base (`:208:9: the ticker's largest gap 1.005346367s; the drop held the runtime's thread 1.000158718s`, now line 210).
  - Outside a runtime: green at both (1.00017 s); red under the mutant (21.8 µs).
- `discarding_the_segment_being_spoken_whose_cancel_cannot_be_written_does_not_block_the_runtime`: red at head and at base.
  - First red, alone at head: `adversary_w2_drop_backed_up.rs:308:9: the discard held the thread 2.001605041s, past CLOSE_BOUND`, with a ticker gap of 2.011904149 s (now line 311).
  - At base: 2.00118249 s.

**`adapters/inference-elevenlabs/tests/adversary_w2_drop_process_exit.rs`** (each case runs the test binary ten times as a child that speaks one segment through the relay, drops the idle handle with a runtime current, drops the runtime and exits; the parent asserts `close_socket` and the close frame of every child)
- `a_process_that_ends_after_a_drop_in_its_runtime_has_sent_close_socket_and_the_close_frame` (drop as the last statement of a current-thread `block_on`): red at head, green at base.
- `a_process_that_ends_after_a_drop_on_an_entered_thread_has_sent_close_socket_and_the_close_frame` (`Runtime::enter`, iterator form): red at head, green at base.
- First red, alone, with one child per case: `adversary_w2_drop_process_exit.rs:83:5: the server received no `close_socket` of the handle dropped inside a runtime: it read [" ", "First. " with flush, close_context], and the relay ClientSide { opcodes: [1, 1, 1], unframed: 0, ended_in_order: true }`.
- With ten children: awaited lost 5, 4 and 2 of 10 in three runs; entered lost 8, 1 and 2 of 10. The assertion is at line 96.

**`adapters/inference-cartesia/tests/adversary_w2_drop_process_exit.rs`**
- `a_process_that_ends_after_a_drop_in_its_runtime_has_sent_the_close_frame`: red at head, green at base.
- `adversary_w2_drop_process_exit.rs:115:9: the server received no close frame of 6 of 10 handles dropped inside a runtime; what the relay read of each: [(0, ClientSide { opcodes: [1], unframed: 0, ended_in_order: true }), ...]` (now line 116); 2 and 1 of 10 in later runs.

The process-exit cases are a race, so one run can pass at head by chance. They were red in 4 of 4 head runs. At base sources, 0 of 330 children lost their close over 11 runs.

**`adapters/inference-elevenlabs/tests/adversary_w2_drop_where.rs`** (green at head and base; these are the attacks that did not break)
- `a_drop_while_the_outer_runtime_shuts_down_still_closes_the_connection` (current-thread and multi-thread)
- `a_drop_while_a_panic_unwinds_inside_a_runtime_still_closes_the_connection`
- `a_drop_on_a_blocking_thread_of_a_runtime_still_closes_the_connection` (`spawn_blocking`, `block_in_place`)
- `forty_drops_inside_a_runtime_leave_no_thread_and_no_descriptor`

## 3. Suite run

Command, in the worktree at head with the five files, after part 2:

```
CARGO_INCREMENTAL=0 cargo nextest run -p inference-elevenlabs -p inference-cartesia --all-features
```

Output (`<scratch>/logs/head-suite-final-2.log`, 358 lines; the failing lines and the summary):

```
slot: synced to build-1d in 0.9 s, 126526 bytes sent, 4 sibling mirrors as base, at 9fbba75ca + 5 changed files
    Starting 242 tests across 50 binaries (7 tests skipped)
    thread '<unnamed>' panicked at adapters/inference-cartesia/tests/adversary_w2_drop_process_exit.rs:116:9:
    the server received no close frame of 1 of 10 handles dropped inside a runtime; what the relay read of each: [(5, ClientSide { opcodes: [1], unframed: 0, ended_in_order: true })]
    thread '..._on_an_entered_thread_...' panicked at adapters/inference-elevenlabs/tests/adversary_w2_drop_process_exit.rs:96:5:
    the server received no `close_socket` and no close frame of 2 of 10 handles dropped inside a runtime; what the relay read of each: [(5, ClientSide { opcodes: [1, 1, 1, 1], unframed: 0, ended_in_order: false }), (9, ClientSide { opcodes: [1, 1, 1], unframed: 0, ended_in_order: true })]
    thread '..._in_its_runtime_...' panicked at adapters/inference-elevenlabs/tests/adversary_w2_drop_process_exit.rs:96:5:
    the server received no `close_socket` and no close frame of 2 of 10 handles dropped inside a runtime; what the relay read of each: [(3, ClientSide { opcodes: [1, 1, 1], unframed: 0, ended_in_order: true }), (5, ClientSide { opcodes: [1, 1, 1], unframed: 0, ended_in_order: true })]
    thread '<unnamed>' panicked at adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs:311:9:
    the discard held the thread 2.001651898s, past CLOSE_BOUND
     Summary [   7.067s] 242 tests run: 238 passed, 4 failed, 7 skipped
        FAIL [   0.578s] ( 82/242) inference-cartesia::adversary_w2_drop_process_exit a_process_that_ends_after_a_drop_in_its_runtime_has_sent_the_close_frame
        FAIL [   0.865s] (138/242) inference-elevenlabs::adversary_w2_drop_process_exit a_process_that_ends_after_a_drop_on_an_entered_thread_has_sent_close_socket_and_the_close_frame
        FAIL [   1.004s] (144/242) inference-elevenlabs::adversary_w2_drop_process_exit a_process_that_ends_after_a_drop_in_its_runtime_has_sent_close_socket_and_the_close_frame
        FAIL [   2.844s] (176/242) inference-cartesia::adversary_w2_drop_backed_up discarding_the_segment_being_spoken_whose_cancel_cannot_be_written_does_not_block_the_runtime
error: test run failed
```

Exit 100. All 230 cases the implementor reported are green in this run. `230` is the implementor's reported count; I did not run the suite before my cases existed.

Other runs of the same command:
- **Head, ten runs** (`<scratch>/logs/head-suite-1..10.log`, earlier versions of my cases): 6.9 to 7.6 s wall, 242 cases each. The unit's four new tests failed 0 times. One existing case failed once (row 7). Two of my own cases flaked from my own mistakes, both fixed: a descriptor baseline taken too early, and a copied `ended_in_order` assertion (row 6).
- **Base sources, ten runs** in `<scratch>/base` (`base-suite-loop-1..10.log`): every run 242 run, 239 passed, 3 failed. The three are my two backed-up drop cases and the discard case, nothing else.
- **Never-waits mutant**, head sources in the scratch copy with the outside-a-runtime branch never taken (`m2-suite-1.log`): 242 run, 236 passed, 6 failed. The six are my two outside-a-runtime cases plus the four red at head anyway. All 230 pre-existing cases pass.

## 4. Findings

| # | file:line | verdict | origin | what was measured | what reaches it |
|---|---|---|---|---|---|
| 1 | `adapters/inference-elevenlabs/tests/async_handles.rs:167` | NEEDS-CHANGE | introduced | `dropping_an_idle_synthesis_does_not_block_the_runtime` passes on base sources (0.203 s), in 11 of 11 base runs. The fixture server has buffer room, so the old waiting drop returns far inside 50 ms. The unit has no test that tells its change from the base. | The gate: reverting the `src` change leaves it green. My backed-up case is red at base (gap 1.002 s) and green at head (12 ms). Its stalled write is built by the case; I use it as the regression test, not as a defect claim. |
| 2 | `adapters/inference-cartesia/tests/cancel.rs:198` | NEEDS-CHANGE | introduced | Same for Cartesia: the unit's test passes at base (0.160 s); my case is red at base (gap 1.005 s), green at head (11.9 ms). | Same as row 1. |
| 3 | `adapters/inference-elevenlabs/src/lib.rs:45` | NEEDS-CHANGE | introduced | The sentence "the close runs to its end on the handle's own thread, and the server still receives `close_socket` and the close frame" does not hold when the process ends after the drop. The spawn at `src/synthesiser.rs:800` returns at once and nothing waits for it. Lost: 5, 4, 2 of 10 (awaited) and 8, 1, 2 of 10 (entered). One child sent `close_socket` and no close frame. Base: 0 of 330 lost. | Any program whose async `main` or runtime ends within about `CLOSE_BOUND` of dropping an idle handle, including a service that drops its sessions at shutdown. On a thread that only entered a runtime (`Handle::enter`), waiting would hold no worker, yet the drop does not wait there either. |
| 4 | `adapters/inference-cartesia/src/lib.rs:31` | NEEDS-CHANGE | introduced | Same for Cartesia ("the server still receives the close frame"; spawn at `src/synthesiser.rs:660`): 6, 2, 1, 1 of 10 children lost the close frame in four runs; base 0 lost. | Same as row 3. Cartesia keeps its connection open until the drop, so every idle handle is in this state. |
| 5 | `adapters/inference-cartesia/src/synthesiser.rs:479` | INFEASIBLE | pre-existing | `discard` of the segment being spoken, inside a runtime, held the runtime's thread 2.0016 s at head and 2.0012 s at base, with a ticker gap of 2.01 s. The cancel write waits `CLOSE_BOUND`, then `close_now` (`:490`) waits another. The docs at `:696` and `src/lib.rs:27` name one `CLOSE_BOUND`. This is the story's kind of block on a path the story does not touch. | Only a connection whose send side is backed up. I built that with a server that stops reading behind 130,000 pings; I cannot show a provider does it. The doc-against-code sum of two bounds holds by reading alone. |
| 6 | `adapters/inference-elevenlabs/tests/async_handles.rs:180` | CONFIRMED | introduced | `ended_in_order == true` is a race. The fixture server answers `close_socket` with its own close frame; if that lands before the client lets go of its socket, the socket ends with a reset and the relay reads an error. Seen once in 50 uses of a copy of this assertion over the same relay, server and close path (`head-suite-9.log`: `ClientSide { opcodes: [1, 1, 1, 1, 8], unframed: 0, ended_in_order: false }`). Not seen in the unit's own test in 12 head runs. | The gate, as a rare flake. `adapters/inference-cartesia/tests/cancel.rs:211` asserts the same; I did not see it fail and did not check whether that fixture server answers a close. |
| 7 | `adapters/inference-elevenlabs/tests/async_handles.rs:112` | CONFIRMED | undecided | `twenty_syntheses_wait_for_the_server_without_blocking_the_runtime` failed once in 12 head suite runs (`head-suite-9.log`: `the ticker's largest gap 55.142673ms`). 0 of 11 at base sources, but those runs took about 13.8 s wall against 7 s, so conditions differ. The unit's `src` change does not touch this test's path: it ends its utterance, so its drops send no close. My ping-flood cases ran in the same suite in both trees and add processor load. | The gate, as a flake of the 50 ms bound under suite load. |
| 8 | `adapters/inference-cartesia/tests/cancel.rs:12` | CONFIRMED | introduced | `#[path = "../../inference-elevenlabs/tests/support/dropped.rs"]` (and `ticker.rs` at `:14`) is the only cross-crate test include in the repository. `cargo package -p inference-cartesia --list --allow-dirty` lists `tests/cancel.rs` and not the two ElevenLabs files, so the packaged crate's `cancel` target cannot compile. A unit gate that builds only `-p inference-elevenlabs` does not compile the Cartesia tests that a change to those files can break. | Packaging or publishing the crate; a later unit that touches only the ElevenLabs support files. My two Cartesia files use the same include. |
| 9 | `adapters/inference-elevenlabs/src/synthesiser.rs:793` | NEEDS-CHANGE | introduced | The new branch "outside a runtime the drop waits" is pinned by none of the 230 cases: the never-waits mutant passes all of them. My `..._outside_a_runtime_waits_close_bound` kills it (16.5 µs against the required wait of `CLOSE_BOUND`). | The doc sentence "Outside an async runtime the drop waits for that close" (`src/lib.rs:44`), which sync callers rely on to have the close sent before they return or exit. |
| 10 | `adapters/inference-cartesia/src/synthesiser.rs:653` | NEEDS-CHANGE | introduced | Same for Cartesia: the mutant survives all pre-existing cases; my case kills it (21.8 µs). | Same as row 9. |
| 11 | `adapters/inference-elevenlabs/tests/adversary_w2_ticker_blocks.rs:181` | CONFIRMED | introduced | The yield case is ignored with a reason that lacks the word `billed`. `AGENTS.md:92-94` says the release check admits an added `#[ignore]` only with that word. I found no such check in `Taskfile.yml`, `.github` or `crates`, and 64 of 81 existing `#[ignore]` lines lack the word too. | Nothing in the repository enforces it; a release check outside the repository would refuse this line if it exists. |
| 12 | `CHANGELOG.md` | NEEDS-CHANGE | introduced | `## [Unreleased]` is empty and the unit's diff does not touch the file. `AGENTS.md:45` puts a consumer-visible change there. The change is consumer-visible: a drop inside a runtime no longer waits, and a process may end before the close is sent. | Consumers reading the changelog at the next release. |

## 5. Attacked and not broken

- **Acceptance where the close cannot be written, at head:** ElevenLabs drop held 49 to 59 µs with a gap of 11.8 to 12.1 ms; Cartesia 51 to 59 µs with a gap of 11.7 to 11.9 ms.
- **The own thread always ends:** it was still there 300 ms after a backed-up drop and gone 1.003 to 1.007 s after it.
- **Many drops:** 500 drops in a row once (26.5 s, then cut to forty) and forty in each later run. At most 1 own thread at once, 0 left, 15 descriptors before and after, `close_socket` on every connection.
- **Outer runtime dropped while a task holds an idle handle** (current-thread and multi-thread): `close_socket` and the close frame arrive and the own thread ends.
- **Drop while a panic unwinds inside a runtime:** same result, no second panic.
- **Drop on a `spawn_blocking` thread and in `block_in_place`:** same result; the drop held 17 to 29 µs.
- **Outside a runtime the drop waits as before:** 1.0001 s at head and at base for both adapters.
- **Ticker correction `39710c30a`:** the helper bodies in `tests/support/ticker.rs` equal those of `async_handles.rs` at `c825c01bc` except for `pub`, one doc paragraph and the removal of `HELD`. The five controls are present; the yield case is ignored with its reason.
- **"A handle whose async form waits sends no close frame":** the repository's protocol notes (`adapters/inference-elevenlabs/tests/fixtures/tts-multi-stream-protocol.md`) say `close_socket` closes all contexts and the connection, and say nothing about an abrupt socket end. No page in the repository asks for a close frame there. The behaviour is the same at base; it was not measured at a provider (no network).
- **Ten runs for flakes:** the unit's four new tests failed 0 times in 12 head suite runs.

## 6. Paths written outside the worktree

- `<scratch>/logs/`: 36 logs.
  - Head: `head-el-new-1.log`, `head-el-new-1.keep.log`, `head-ca-new-1.log`, `head-new-2.log`, `head-new-3.log`, `head-suite-1.log` to `head-suite-10.log`, `head-suite-final.log`, `head-suite-final-2.log`, `head-fmt.log`, `head-clippy.log`, `head-clippy-2.log`.
  - Base: `base-el-new-1.log`, `base-ca-new-1.log`, `base-new-2.log`, `base-suite-1.log`, `base-suite-loop-1.log` to `base-suite-loop-10.log`.
  - Mutant: `m2-suite-1.log`.
- `<scratch>/base/`: a `git archive` copy of head without `.engineering`, `website` and `ux`. It held the base versions of the four changed `src` files for the base runs. It now holds head `lib.rs` and the never-waits mutant in the two `synthesiser.rs`, so it is no longer base. Its build output is on the build node, not on this host.
- `<scratch>/m2/elevenlabs-synthesiser.rs` and `<scratch>/m2/cartesia-synthesiser.rs`: the mutant sources.
- `<scratch>/ten-runs.sh`, `<scratch>/final-runs.sh`, `<scratch>/closing-head.sh`: run scripts.
- `<scratch>/async_handles.c825c01bc.rs`, `<scratch>/old-helpers.rs`, `<scratch>/new-helpers.rs`: the ticker helper comparison.

## 7. Findings block

```findings
- file: adapters/inference-elevenlabs/tests/async_handles.rs
  line: 167
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the unit's acceptance test of the drop passes on the base sources, so no test tells the change from the base; adversary_w2_drop_backed_up.rs is red at base (gap 1.002 s) and green at head
- file: adapters/inference-cartesia/tests/cancel.rs
  line: 198
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the Cartesia acceptance test of the drop passes on the base sources; the backed-up case is red at base (gap 1.005 s) and green at head
- file: adapters/inference-elevenlabs/src/lib.rs
  line: 45
  category: contract-drift
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the documentation says the server still receives close_socket and the close frame after a drop inside a runtime, but a process that ends after the drop lost them for 1 to 8 of 10 children per run, against 0 of 330 at base
- file: adapters/inference-cartesia/src/lib.rs
  line: 31
  category: contract-drift
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the documentation says the server still receives the close frame after a drop inside a runtime, but a process that ends after the drop lost it for 1 to 6 of 10 children per run, against 0 lost at base
- file: adapters/inference-cartesia/src/synthesiser.rs
  line: 479
  category: boundary
  severity: warning
  verdict: INFEASIBLE
  origin: pre-existing
  message: discard of the segment being spoken holds the caller's thread 2.0 s inside a runtime when the cancel cannot be written, twice the documented CLOSE_BOUND, in a backed-up state the case constructs
- file: adapters/inference-elevenlabs/tests/async_handles.rs
  line: 180
  category: concurrency
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the ended_in_order assertion races with the fixture server's own close frame; a copy of it over the same relay and close path failed once in 50 uses
- file: adapters/inference-elevenlabs/tests/async_handles.rs
  line: 112
  category: concurrency
  severity: warning
  verdict: CONFIRMED
  origin: undecided
  message: twenty_syntheses_wait_for_the_server_without_blocking_the_runtime failed once in 12 head suite runs with a gap of 55.1 ms against the 50 ms bound; 0 of 11 at base under different run conditions
- file: adapters/inference-cartesia/tests/cancel.rs
  line: 12
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the test includes two files of another crate by relative path, the only such include in the repository; the packaged crate does not carry them and a per-crate gate of the other adapter does not compile this test
- file: adapters/inference-elevenlabs/src/synthesiser.rs
  line: 793
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a drop that never waits outside a runtime passes all 230 pre-existing cases; only the added outside-a-runtime case kills it
- file: adapters/inference-cartesia/src/synthesiser.rs
  line: 653
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a drop that never waits outside a runtime passes all pre-existing Cartesia cases; only the added outside-a-runtime case kills it
- file: adapters/inference-elevenlabs/tests/adversary_w2_ticker_blocks.rs
  line: 181
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the added ignore reason lacks the word billed that AGENTS.md says the release check requires, and no such check exists in the repository
- file: CHANGELOG.md
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the Unreleased section has no entry for the consumer-visible change that a drop inside a runtime no longer waits for its close
```

