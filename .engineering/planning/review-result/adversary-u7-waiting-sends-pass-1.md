---
format: aep.planning-md/3
id: review-result:adversary-u7-waiting-sends-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on waiting send order: no red case, four notes on an unpolled granted place, the residual, a dead branch and a panicking waker'
relations:
- reviews: story:waiting-sends-land-in-the-order-they-started-waiting
revision: 1
---
```
unit: story:waiting-sends-land-in-the-order-they-started-waiting, tree inference-u7-waiting-sends at 93ab0ee3 (the unit's head), with my cases committed on top as ae789d7a
verdict: CONFIRMED (four findings, all `note` severity; no case is red)
cases: executed 267→277, red 0
origin: introduced 3 / pre-existing 0 / undecided 1
wrote-outside-worktree: 13
needs-coordinator: none
```

Nothing I attacked turned red. The queue does what the design says, and the accepted residual is the only reordering I could reach. I committed the cases anyway, under your rule, because each one catches a change to `queue.rs` that no existing case covers. I worked that out by reading; I did not build those mutated versions of `queue.rs`.

### 1. What I touched (`git --no-pager diff --stat 93ab0ee3..HEAD`)
```
 .../tests/adversary_u7_threads.rs                  | 274 +++++++
 .../tests/adversary_u7_waiting_sends.rs            | 791 +++++++++++++++++++++
 2 files changed, 1065 insertions(+)
```
Only test files. Commit ae789d7a went through the bot route with the requested subject and a body. `git status` is clean.

### 2. Cases added (all green on the first run)
All of them were written before anything ran. The first run was these two targets alone. Log: `.../adv1/red1.log`, `test result: ok. 2 passed` and `ok. 8 passed`.

In `crates/inference-runtime/tests/adversary_u7_waiting_sends.rs`:
- **Granted place given up before its send polls:** the place goes to the next waiting send, that send is woken and lands, and a later send is not held up. This catches `withdraw` losing either its grant or its wake.
- **Passed send gets room again:** it is granted room before a send that arrives after the pass, the queue stays within capacity, and the received order is `[3,5,6]`. This catches a pass that leaves the ticket marked as placed; that version yields `[5,6,3]` and holds 3 envelopes in a queue of capacity 2.
- **Send polled again with a new waker:** it is woken through the new one. This catches a ticket that keeps its first waker.
- **Raise makes a placed send stale:** its place goes to the next waiting send, and the stale send is counted as discarded exactly once. This catches a raise that skips `remove_stale_places`.
- **Close with landed, granted and waiting sends:** the close answers `Holding`, the receiver hands over 3 and 5 and then `None`, every unlanded send is refused with its own envelope, and nothing is counted as discarded.
- **Seeded schedule model:** 5 policies × capacities 1 to 3 × three schedules (`Prompt`, `WokenPrompt`, `WokenOnly`) × 3 endings × 150 seeds. At every step it checks:
  - the queue never holds more than its capacity;
  - `known_contents` matches what `recv` does;
  - every envelope ends exactly once as received, refused with itself, withdrawn, or counted in `discarded()`;
  - no lost wakeup and no room held that no send can use;
  - no waker storm.
  - Order: under `Prompt` and `WokenPrompt`, envelopes arrive in the order their sends started.
  - A reach check shows the schedules really made sends wait, give up, and land out of order under `WokenOnly`.

In `crates/inference-runtime/tests/adversary_u7_threads.rs`: 4 producer threads × 300 sends, every fifth one given up after it starts waiting. One case reads to the end; the other drains by `known_contents` and then closes. Both run under a 60 s deadline.

Repeat runs: `adversary_u7_threads: 0 failures in 50`, `adversary_u7_waiting_sends: 0 failures in 50`.

### 3. Package suite (run after the cases existed)
`nice cargo test -j 8 -p inference-runtime`: exit 0, 62 test binaries, `passed 277 failed 0 ignored 0`. My two files account for 10 of those, so `<before>` = 267, worked out by subtraction from this one run. `cargo clippy -p inference-runtime --all-targets --all-features -- -D warnings` exits 0 after a `% n == 0` → `is_multiple_of` fix. The final re-run after formatting is still green.

### 4. Findings (covering ae789d7a)

| file:line | what | verdict | origin |
|---|---|---|---|
| crates/inference-runtime/src/queue.rs:292 | A send that was granted a place and is kept alive but not polled holds that room. Younger waiting sends and a receiver blocked in `recv` wait on its caller, where the base would have delivered the younger send. What reaches it: nothing found; callers either poll a send or drop it. | INFEASIBLE | introduced |
| crates/inference-runtime/src/queue.rs:583 | The residual (passing a place) still lets two sends in flight on one stream reach the consumer as 6 then 5 through the U6 guarded sender, which had admitted both. What reaches it: no caller makes concurrent sends on one sender. The base reorders here too, but I read that rather than ran it. | CONFIRMED | undecided |
| crates/inference-runtime/src/queue.rs:509 | Dead code: the `remove_place` and `grant` in `poll_ticket`'s stale branch never run, because every raise strips stale places first (:427). Changing them alters nothing, so no test can pin them. | CONFIRMED | introduced |
| crates/inference-runtime/src/queue.rs:565 | `Sending::drop` now wakes other tasks. If one of those wakers panics, U6's `Underway::drop` (drive.rs:503 at bc841d7a) runs during the unwind and keeps as taken a sequence the queue never took. The result is conservative: a resend of that sequence is refused. What reaches it: only a waker that panics. | INFEASIBLE | introduced |

**U6 guarded sender, by reading:** FIFO grants break none of its assumptions. A send granted a place and dropped before it lands goes through `withdraw`: its room passes on, nothing was taken, and U6 correctly gives the sequence back. An envelope still enters the queue only during a poll of its own send that answers `Ready(Ok)`; the one exception is a wake of the receiver that panics after landing, which U6 already treats as taken. A send refused as `Closed` at a close or at the receiver's drop settles as not taken, which is also correct. The last two rows are the only interactions I found. I could not build the combined code.

### 5. Attacked and not broken
- A granted place whose send is dropped: the room goes to the next ticket and is never left stalled (case, model, threads).
- Raising and lowering capacity: not possible, because capacity is a fixed `NonZeroUsize` and nothing sets it. I attacked epoch raises instead (case and model).
- Closing to senders, and dropping the receiver, with granted unlanded places: every waiting send is woken and gets its own envelope back.
- Several streams on one queue, and the `fail`, `drop` and `coalesce` policies: capacity, truthfulness and accounting all hold.
- The drain's view: `known_contents` is never `NotEmpty` unless `recv` hands an envelope over at once.
- Lost wakeups and waker storms: none in the model; none in 50×2 threaded runs.
- The residual: under prompt or woken-prompt polling the order is exactly the start order. The only inversions appear under `WokenOnly`, which is the residual.

### 6. Paths written outside the worktree (13)
All in the unit's `adv1/` scratch directory:
- the directory itself
- `before-reach.rs.txt`
- `clippy.log`
- `clippy2.log`
- `commit-msg.txt`
- `final.log`
- `loop50.log`
- `loop50.sh`
- `loop-last.log`
- `red1.log`
- `run2.log`
- `run3.log`
- `suite.log`

The lease `inference-u7-adv1` is released.

### 7. Findings block
```findings
[
  {"file": "crates/inference-runtime/src/queue.rs", "line": 292, "category": "judgement", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "A granted place whose send stays alive but unpolled holds the room, so younger waiting sends and a receiver blocked in recv wait on that caller, where the base delivered the younger send; no caller found that keeps a send unpolled."},
  {"file": "crates/inference-runtime/src/queue.rs", "line": 583, "category": "concurrency", "severity": "note", "verdict": "CONFIRMED", "origin": "undecided", "message": "The accepted residual still lets two in-flight sends on one stream reach the consumer out of sequence past the U6 guard, which admitted both; no caller makes concurrent sends on one sender."},
  {"file": "crates/inference-runtime/src/queue.rs", "line": 509, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "The remove_place and grant in poll_ticket's stale branch never run because every raise removes stale places first at :427, so changing them alters nothing and no test can pin them."},
  {"file": "crates/inference-runtime/src/queue.rs", "line": 565, "category": "judgement", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "Sending::drop now wakes other tasks, so a panicking waker makes U6's Underway::drop keep as taken a sequence the queue never took; only a panicking waker reaches it."}
]
```