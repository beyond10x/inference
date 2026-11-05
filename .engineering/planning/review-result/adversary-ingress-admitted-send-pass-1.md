---
format: aep.planning-md/3
id: review-result:adversary-ingress-admitted-send-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the admitted send at a drain: a handed-in send is delivered after its caller gave up; the admission race reproduced'
relations:
- reviews: story:run-ingress-admitted-send-outlives-the-drain
revision: 1
---
## Pass

Adversary pass 1 on story:run-ingress-admitted-send-outlives-the-drain, against `df0e382` on
`impl/run-ingress-admitted-send-outlives-the-drain`; the cases are `3b3c0df`,
`crates/inference-runtime/tests/adversary1_parked_send_queue.rs` and
`crates/inference-runtime/tests/adversary1_parked_send_run.rs` (test files only). The red cases
were also run on a copy of the base `142db68` to settle their origin.

verdict: NEEDS-CHANGE
cases: executed 238→249, red 4
origin: introduced 1 / pre-existing 3 / undecided 0

## Cases

- `a_send_given_up_by_its_caller_after_the_take_made_room_is_not_delivered`: RED, green on the
  base. "the consumer later received [2], and the caller had no sign of it".
- The admission race: RED on both. A send passes `admits_input`, is held before the queue lock
  (the epoch lookup under `drop_stale_epochs` gives the hook), the bridge finds the parent's
  ingress empty and sends `stop`, the held send lands and answers Ok: "the child's node received
  [\"stop\"]". Under `pause` the window is the same and reached only by thread timing.
- The retry case: RED on both. A send dropped while waiting is withdrawn but its stream sequence
  stays advanced, so the same envelope sent again is refused `SequenceNotIncreasing { sequence:
  2, last: 2 }`.
- `a_send_arriving_after_a_parked_one_is_not_taken_before_it_once_a_raise_made_room`: RED on both,
  under `drop_stale_epochs` only.
- Green: a send dropped while parked delivers nothing; a parked send answers `Closed` with its
  envelope when the receiver goes, never both returned and delivered (threaded, 4×300); a handed-in
  send before the receiver goes is counted as discarded; parked sends land in parking order with
  their latest wakers woken; a stale parked send completes as discarded; streams stay in order
  (threaded, 4×500, capacity 2).
- Suites: `inference-runtime --lib --tests` 245 passed, 4 failed (the four above);
  `inference-facade` 69 passed; clippy and fmt clean.

## Routing

The introduced finding and the admission race, which is the story's own acceptance, go back to
the unit as its correction. The retry case and the overtaking under `drop_stale_epochs` are
pre-existing and filed as drafts; their red cases stay on the unit branch, not merged.

```findings
[{"file":"crates/inference-runtime/src/drive.rs","line":437,"category":"acceptance","severity":"blocker","verdict":"CONFIRMED","origin":"pre-existing","message":"an ingress send admitted before the drain that reaches the queue after the subgraph bridge judged it empty answers Ok and is never delivered (adversary1_parked_send_run.rs:434)"},{"file":"crates/inference-runtime/src/queue.rs","line":748,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"a send dropped by its caller after a take handed its envelope in is delivered with no sign to the caller, where before a dropped unfinished send delivered nothing (adversary1_parked_send_queue.rs:206)"},{"file":"crates/inference-runtime/src/drive.rs","line":442,"category":"contract-drift","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"a send withdrawn by its drop keeps its sequence advanced, so the same envelope sent again is refused as SequenceNotIncreasing for a sequence the edge never took (adversary1_parked_send_run.rs:542)"},{"file":"crates/inference-runtime/src/queue.rs","line":351,"category":"concurrency","severity":"note","verdict":"INFEASIBLE","origin":"pre-existing","message":"under drop_stale_epochs a raise frees room and a later send is queued ahead of an already parked one; no caller in the crate sends concurrently on one sender (adversary1_parked_send_queue.rs:318)"}]
```
