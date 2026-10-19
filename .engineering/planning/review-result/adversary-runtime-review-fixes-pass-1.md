---
format: aep.planning-md/3
id: review-result:adversary-runtime-review-fixes-pass-1
kind: review-result
status: active
title: Adversary, story:runtime-review-fixes, pass 1
relations:
- reviews: story:runtime-review-fixes
revision: 1
---
```
unit: story:runtime-review-fixes, uncommitted working tree on base 5f40904 (worktree inference-impl-runtime-review-fixes)
verdict: nothing found
cases: executed 21→26, red 0
origin: introduced 0 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory (scratch/adversary-1, 6 files; listed in part 6)
needs-coordinator: none
```

**1. `git --no-pager diff --stat`**: unchanged from the implementor's 5 files (113+/29−). My one addition is untracked: `crates/inference-runtime/tests/adversary1_review_fixes.rs`. It is a test file. I touched no `src/`, `ess/` or manifest file, so there is no charter violation.

**2. Cases added** (`<trees>/inference-impl-runtime-review-fixes/crates/inference-runtime/tests/adversary1_review_fixes.rs`). All 5 are green now and were green on their first run alone (`own-cases.log`, `5 passed; 0 failed`). None is a finding; they guard the fix.

| Case | What it asserts |
|---|---|
| `a_raised_epoch_completes_every_stale_pending_send_and_keeps_the_current_one_waiting_for_a_take` | Two stale pending sends of producer a both finish as discarded with no consumer pop. A current-epoch pending send of producer c is pulled off the wait list by the raise and re-registers. It is then woken by the next take (no lost wake), and so is the raiser. Final delivery order and discarded count are checked exactly. |
| `a_purge_that_frees_room_lets_a_woken_current_epoch_send_in_without_a_take` | On the room path: the purge frees 3 slots, the stale waiter finishes as discarded and the current waiter is accepted. End state is `(len, discarded) = (2, 4)`. |
| `coalesce_matches_the_decision_on_a_random_sequence_and_accounts_for_every_send` | Fixed seed, capacities 1 to 4, 3000 random sends and pops over 3 streams. After every step `len` and `discarded` must equal a model of the story's rule. Every pop must equal the model's head. Delivered + discarded must equal sent, and each stream must stay in order. |
| `dropping_the_receiver_counts_what_it_clears_under_every_policy` | For all 5 policies with a full queue: dropping the receiver adds exactly the queued count. Delivered + discarded must equal accepted sends. A pending send is woken and gets `Closed` back with its own envelope. |
| `drop_stale_epochs_with_racing_epochs_of_one_node_finishes_and_accounts_for_every_send` | 3 producers × 4 epoch threads each, all running at once, capacity 2, slow consumer, 60 s deadline. Delivered + discarded must equal all sends. No producer's deliveries go back to an older epoch, and the last delivered epoch is the newest. |

**3. Suite run, after my cases existed** (logs in scratch):
- `cargo test -p inference-runtime --all-features --no-fail-fast`: TEST_EXIT=0. Per binary: 0 / 1 / 5 / 3 / 7 / 10 / 0 passed, 0 failed. That is 26 executed; before my file it was 21 (from the implementor's `green-runtime.log`).
- `cargo clippy -p inference-runtime --all-targets --all-features -- -D warnings`: CLIPPY_EXIT=0.
- `cargo fmt -p inference-runtime --check`: FMT_EXIT=1 on the first run. All 7 diffs were in my own new file. I ran `rustfmt` on that file, and the re-run gave FMT_EXIT=0; the test and clippy re-runs also exited 0.

**4. Judgement findings**: none raised.
- **The changed assertion in `bounded_edges.rs` (`coalesce_replaces_the_newest_item_of_the_same_stream`) is correct.** The sends go [1,2,3,8] at capacity 4, then a send on another stream evicts item 1. The expected `[2,3,8,9]` and `discarded = c+1` follow the story's Decision exactly. Design line 684 ("Coalesce/supersede old hypotheses when safe") supports keeping the newest over the oldest. It is not a weakening.
- **Residue I did not raise:** module doc `queue.rs:6` says "the receiver takes envelopes in arrival order". Replacing an envelope in place under coalesce reorders across streams, but line 14 says "in place" explicitly, and both lines were already at the base. It is a doc nit, so I wrote no red case.
- **Already handled:** `Cargo.toml` still describes the crate as "one bounded queue per edge per run". The implementor already left `scratch/runtime-cargo-description.patch` for it.

**5. Attacked and could not break.** I ran 10 mutants of `queue.rs` on a scratch copy (`mutants.log`). The unit's own cases kill every one of them:

| Mutant | Unit cases that killed it | Mine as well |
|---|---|---|
| M1: wake waiting senders only when the purge removed something (old behaviour) | `a_newer_epoch_completes…` | 1 |
| M2: no wake on the full path | `a_newer_epoch_completes…` | 2, threads case included |
| M3: no wake on the room path | `drop_stale_epochs_waits_when_full…` | 2, threads case included |
| M4: coalesce evicts the newest (`pop_back`) | 2 | 1 |
| M5: receiver drop not counted | `dropping_the_receiver_counts_the_envelopes_it_clears` | 1 |
| M6: coalesce throws away the arrival (old behaviour) | 2 | 1 |
| M7: coalesce eviction not counted | 4 | 1 |
| M8: purge not counted | 5 | 1 |
| M9: stale arrival not counted | 4 | 2 |
| M10: receiver drop counts one less | 1 | 1 |

Other ground with no break:
- Every exit path of `poll_send` after a raise wakes the waiting senders.
- The receiver-closed check and the stale check both return before any raise.
- A task holding two pending sends is woken through `register`'s de-duplication.

**6. Paths written outside the worktree.** All are under `<unit scratch>/adversary-1/`: `mutate.py`, `mutants.log`, `own-cases.log`, `suite.log`, `clippy.log`, `fmt.log`. The mutant copy `mut/` (its `target/` and per-mutant logs included) was deleted once its run was logged; I also deleted two helper scripts. My lease is released.

```findings
[]
```