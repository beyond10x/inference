---
format: aep.planning-md/3
id: review-result:adversary-u6-withdrawn-ingress-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on withdrawn ingress sends: a send unwinding after the queue took its envelope gives its sequence back'
relations:
- reviews: story:withdrawn-ingress-send-gives-back-its-sequence
revision: 1
---
```
unit: story:withdrawn-ingress-send-gives-back-its-sequence, findings cover 454b56e6 (cases committed as 4f322a08)
verdict: INFEASIBLE
cases: executed 267→272, red 1
origin: introduced 1 / pre-existing 0 / undecided 0
wrote-outside-worktree: 27 paths (1 directory, 26 files)
needs-coordinator: decide whether the unwind case holds the unit or goes to the backlog (severity note)
```

1. **Diff stat** (454b56e6..HEAD). Every path is a test file.
```
 .../tests/adversary_u6_sequences.rs                | 537 +++++++++++++++++++++
 1 file changed, 537 insertions(+)
```
Mutants were tested by editing `crates/inference-runtime/src/drive.rs` in the worktree, as the coordinator allowed; the adversary procedure forbids that. Each mutant was restored with `git checkout -- <file>` straight after its run. `git status` was clean apart from the test file before the commit.

2. **Cases added** in `crates/inference-runtime/tests/adversary_u6_sequences.rs`. First run of that file alone (`cargo test -p inference-runtime --test adversary_u6_sequences`):
- `sends_under_way_given_up_in_every_order_give_back_exactly_their_own_sequence` (:332): four sends wait at once and are given up in all 24 orders. After each give-up, the exact refusal or admission is checked, then the oldest is resent. Green.
- `a_duplicate_of_a_send_still_under_way_is_refused_naming_its_sequence` (:386): a duplicate sent while the first is still waiting gets `Refused(last 2)`, and refusals are `[(2, 2)]`. Green.
- `two_waiting_sends_taken_in_either_order_leave_the_higher_sequence_last` (:428): after both are taken, resending sequence 3 gets `Refused(last 3)`. Green.
- `a_send_refused_as_full_gives_back_its_sequence` (:475): under `fail`, a send refused as Full can be sent again and is taken. Green.
- `a_send_that_unwinds_after_the_queue_took_its_envelope_keeps_its_sequence` (:506): **red**.
```
thread 'a_send_that_unwinds_after_the_queue_took_its_envelope_keeps_its_sequence' (42162) panicked at crates/inference-runtime/tests/adversary_u6_sequences.rs:529:5:
the queue took `first` at sequence 1, yet the same envelope sent again answered Ok and the node then received Some("first")
test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
```
(:529 moved to :532 after `cargo fmt`.)

3. **Suite run**: `nice cargo test -j 8 -p inference-runtime --no-fail-fast`, exit 101: 271 passed, 1 failed (the unwind case). The "before" count of 267 is that same run minus the 5 new cases. `cargo clippy -p inference-runtime --all-targets --all-features -- -D warnings` exits 0.

Origin: the file was run against the base `drive.rs` (`git show ff0ba010:…` written into the worktree, then restored). On the base only the 24-orders case fails, which is the defect this unit fixes. The unwind case passes there, so it is **introduced**.

**Mutants of the new lines.** Targeted set: `--lib` plus the two parked-send files, `refusal_and_recorder_bounds`, `text_pipeline`, `adversary1_text_pipeline` and the new file. Two mutants survived every pre-existing test in that set and are now caught only by the new cases; they were not run against the full package suite.
- M1, `drive.rs:485`: `stream.taken = stream.taken.max(Some(sequence))` changed to `= Some(sequence)`. Caught only by `two_waiting_sends_taken_in_either_order…`. The case relies on main's queue waking all waiting senders, so a newer send can be taken before an older one. If the FIFO queue unit lands, this mutant may become equivalent.
- M4, `drive.rs:423`: `last()` taking `.min()` instead of `.max()`. Caught only by the duplicate case and the 24-orders case.

M5–M11 (equality test in `settle`, an empty Drop body, `settle(.., true)`, `<=` changed to `<`, dropping `chain(taken)`, `if true`, no explicit settle) are each caught by tests that already existed.

4. **Findings**

| file:line | what | verdict | origin |
|---|---|---|---|
| crates/inference-runtime/src/drive.rs:500 | `Underway::drop` gives the sequence back when a send unwinds after `poll_send` pushed the envelope. The panic comes from `wake(receiver)` or `wake(senders)` in `queue.rs` after the push. The same envelope sent again is then taken, so the node gets it twice. Measured at `adversary_u6_sequences.rs:532` (exit 101); it passes against the base `drive.rs`. | INFEASIBLE | introduced |

What reaches it: only a waker that panics, either the node input's waker or another sender's waker. No such waker was found in the repository's executors, so the state is built by the test. The doc on `Underway` ("has never pushed its envelope") is false while unwinding. Suggested fix (not applied): skip the give-back in `Drop` when `std::thread::panicking()`, which keeps the base's behaviour of keeping the sequence on unwind.

5. **Attacked, could not break**
- Give-up order, oldest first, newest first or mixed: each send gives back exactly its own sequence, and `last()` never drops below a taken one.
- A sequence used by two envelopes: no duplicate is ever under way, and a taken sequence is not freed except by the unwind case.
- Push versus cancel without a panic: the push and `settle` happen in the same poll, so there is no window to drop the future in between.
- Closed queue and drain: `admits_input` refuses before `advance`, and a Closed or Full refusal settles untaken.
- Sharing across tasks: `GuardedSender` is not `Clone`. Sharing goes through `&self`, and each `advance`/`settle` takes the mutex once. Checked by reading only.
- `SequenceNotIncreasing` for a well-behaved caller: exact in all 24 orders.
- Out of scope, pre-existing: on main, a lower sequence can reach the node after a higher one when two sends wait on one stream. That belongs to the queue-order unit.

6. **Paths written outside the worktree**: 27, all in the unit's `adv1/` scratch directory (the directory, the base diff and log, case, suite and clippy logs, the mutant script and table, nine mutant diffs and logs, the commit message).

7. Findings block:
```findings
[
  {"file": "crates/inference-runtime/src/drive.rs", "line": 500, "category": "concurrency", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "A send that unwinds from a panicking waker after the queue pushed its envelope gives its sequence back through the Underway drop guard, so the same envelope is taken and delivered twice; the base kept the sequence on unwind."}
]
```
