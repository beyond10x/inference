---
format: aep.planning-md/3
id: review-result:adversary-bounded-edges-pass-1
kind: review-result
status: active
title: Adversary, story:bounded-edges, pass 1
relations:
- reviews: story:bounded-edges
revision: 1
---
```
unit: story:bounded-edges, working tree of impl/bounded-edges (base 4a96264 plus the uncommitted change)
verdict: NEEDS-CHANGE
cases: executed 55→65 (inference-runtime 8→18, inference-core 47→47), red 1
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 path (scratch dir adversary-1/, logs and the mutation scripts; the mutant copy is deleted)
needs-coordinator: none
```

**Verdict:** one red case. A capacity that `ConnectPorts` accepts makes `EdgeQueue::new` panic. I found no concurrency defect, but the unit's own suite cannot detect a lost wakeup.

## 1. What I touched

`git --no-pager diff --stat` lists only the unit's tracked changes (`Cargo.lock` +8, `crates/inference-core/src/lib.rs` +1), because the crate is untracked. `git status -uall` shows my three new files, all test files:

- `crates/inference-runtime/tests/adversary1_capacity.rs`
- `crates/inference-runtime/tests/adversary1_threads.rs`
- `crates/inference-runtime/tests/adversary1_wakeups.rs`

No implementation file, manifest or `ess/` path changed. I wrote no `adversary5_endpoint_*` file (see part 4, J2).

## 2. Cases added (each run alone before the suite)

| file:line | case | now |
|---|---|---|
| adversary1_capacity.rs:52 | an edge with capacity `i64::MAX`, the largest value `ConnectPorts` accepts, builds a queue without panicking | **red** |
| adversary1_wakeups.rs:125 | under pause, a waiting producer is woken when the consumer takes one | green |
| adversary1_wakeups.rs:139 | a send wakes a waiting consumer | green |
| adversary1_wakeups.rs:153 | dropping one of two senders does not end the edge; dropping the last wakes the consumer | green |
| adversary1_wakeups.rs:170 | dropping the receiver wakes a waiting producer, which gets `Closed` back | green |
| adversary1_wakeups.rs:186 | `drop_stale_epochs` on a full queue makes the producer wait; a newer epoch purges, wakes the waiter, and its stale item is discarded | green |
| adversary1_wakeups.rs:219 | `drop_stale_epochs` compares epochs per producer | green |
| adversary1_threads.rs:177/187/197 | 4 producer threads × 500 items with a park-based `block_on` and a 60 s deadline: pause and `drop_stale_epochs` lose nothing and keep each stream in order; drop and coalesce stay within capacity, delivered + discarded = 2000, streams stay in order | green (30 extra runs, 0 failures) |

Red output, captured when the case was first run (`cargo test -p inference-runtime --all-features --test adversary1_capacity`, EXIT=101):
```
thread 'the_largest_capacity_connect_ports_accepts_builds_a_queue' panicked at .../alloc/src/raw_vec/mod.rs:28:5:
capacity overflow
thread '...' panicked at crates/inference-runtime/tests/adversary1_capacity.rs:62:5:
EdgeQueue::new panicked for a declared capacity of 9223372036854775807
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

## 3. Suite and gate (run after the cases existed, each with its own exit status)

| command | exit | summary |
|---|---|---|
| `cargo test -p inference-runtime --all-features --no-fail-fast` | 101 | adversary1_capacity `FAILED. 0 passed; 1 failed`; adversary1_threads `ok. 3 passed`; adversary1_wakeups `ok. 6 passed`; bounded_edges `ok. 8 passed` |
| `cargo test -p inference-core --all-features --no-fail-fast` | 0 | 15 binaries, 47 passed |
| `cargo clippy -p inference-runtime --all-targets --all-features -- -D warnings` | 0 | |
| `cargo fmt -p inference-runtime --check` | 0 | |

The first clippy and fmt runs failed on my own test files (`type_complexity`, formatting). I fixed those files and the commands above are the reruns.

## 4. Findings

**F1: building a queue reserves memory for its full capacity up front.** `queue.rs:277` calls `VecDeque::with_capacity(edge.capacity().get())`.
- *Measured:* adversary1_capacity.rs:62 is red; capacity `i64::MAX` panics with "capacity overflow".
- *Inferred, not run:* a capacity like 10⁹ would not overflow but would try to allocate capacity × one queued envelope, and an allocation failure aborts the process.
- *What reaches it:* `ConnectPorts` accepts any capacity ≥ 1 and maps `i64` to `NonZeroUsize` (`declaration.rs:472-478`). The builder's `connect` accepts any `NonZeroUsize`. ESS declares no upper bound (graph.yaml:433, "UNMAPPED: an upper bound"). Nothing outside tests calls `EdgeQueue::new` yet; the per-run wiring belongs to story:node-instance-lifecycle.
- *Fix (not applied):* `VecDeque::new()`. With that mutant (M9) applied in the scratch copy, the red case turns green and bounded_edges stays green.

**F2: the unit's suite cannot fail on a lost wakeup.** `bounded_edges.rs:93-95` polls every future with `Waker::noop()`, and the pause test re-polls on every tick whether woken or not. I applied each mutant to a scratch copy of `queue.rs`:

| mutant | bounded_edges | my cases that fail |
|---|---|---|
| M1 recv wakes no waiting sender | green | wakeups:125, threads:177 and :187 (60 s deadline) |
| M2 send wakes no waiting receiver | green | wakeups:139, threads:177 and :187 |
| M6 receiver drop wakes nobody | green | wakeups:170 |
| M7 epoch purge wakes nobody | green | wakeups:186 |
| M8 last sender drop wakes nobody | green | wakeups:153 |

**F3: three policy and lifecycle paths are untested.**

| mutant | bounded_edges | my cases that fail |
|---|---|---|
| M3 epoch purge ignores the producer | green | wakeups:219 |
| M4 full queue under `drop_stale_epochs` discards instead of waiting | green | wakeups:186, threads:187 |
| M5 cloning a sender does not count it | green | wakeups:153 and :186, threads (sender count underflows) |

The reason: bounded_edges' `drop_stale_epochs` cases use one producer and never fill the queue, and no case clones a sender. F2 and F3 are now guarded by the cases in part 2.

**J1 (judgement, not run):** `endpoint.rs:29`: `OutputEndpoint<P>: Send` without `Sync`, while `send` takes `&self`. So an `Arc<dyn OutputEndpoint<P>>` is not `Send`, and a node cannot share one output endpoint across tasks on a multi-threaded executor. *What reaches it:* nothing found; the node trait does not exist yet. This is a type rule, not something I measured.

**J2:** I wrote no endpoint test file. The only endpoint property I found (J1) is compile-time, and a case that fails by not compiling does not count as a finding.

## 5. What I attacked and could not break

- Capacity under concurrent senders: the mutex holds; 30 extra runs of 4 threads at capacity 2–3 never exceeded it.
- Lost wakeups in the implementation: every path I tested wakes the waiting task.
- Per-stream order under coalesce and drop: replacing in place keeps each stream in order (design §19.2).
- Acceptance checks 1–5 against the story text, and the overload policy enum order against graph.yaml:80 and `OverloadPolicy::ALL`: both match.
- `cancel`/`supersede` refused at build time, and `Closed` returned to a waiting sender: both hold.
- A stale sender that is waiting gets its item discarded on re-poll: holds.

## 6. Paths written outside the worktree

`<unit scratch>/adversary-1/` holds `red-capacity.log`, `guards.log`, `suite-runtime.log`, `suite-core.log`, `clippy-runtime.log`, `fmt-runtime.log`, `mutants.log`, `mutants.sh`, `mutate.py`, and `M1…M9-*.{unit,adv}.log`. The mutant copy `adversary-1/mut/` and its `target/` (95M) are deleted. My lease is ended with session-end; disk is at 50G free.

## 7. Findings block

```findings
- file: crates/inference-runtime/src/queue.rs
  line: 277
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: EdgeQueue::new preallocates the declared capacity, so a capacity ConnectPorts accepts (i64::MAX) panics with capacity overflow and a large finite one tries to allocate the whole bound at build time; VecDeque::new() fixes it.
- file: crates/inference-runtime/tests/bounded_edges.rs
  line: 94
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: every unit case polls with Waker::noop and re-polls unconditionally, so five lost-wakeup mutants (M1, M2, M6, M7, M8) leave the unit suite green; adversary1_wakeups and adversary1_threads now kill them.
- file: crates/inference-runtime/tests/bounded_edges.rs
  line: 197
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: no unit case covers drop_stale_epochs with two producers or a full queue, or a cloned sender, so mutants M3, M4 and M5 survive; adversary1_wakeups:153, :186 and :219 now kill them.
- file: crates/inference-core/src/endpoint.rs
  line: 29
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: OutputEndpoint is Send but not Sync while send takes &self, so Arc<dyn OutputEndpoint<P>> is not Send; no node trait consumes it yet.
```
