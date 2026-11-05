---
format: aep.planning-md/3
id: review-result:adversary-ingress-admitted-send-pass-2
kind: review-result
status: active
title: 'Adversary pass 2 on the admitted send at a drain: a reserved envelope taken before its send is polled again is delivered after a give-up'
relations:
- reviews: story:run-ingress-admitted-send-outlives-the-drain
revision: 1
---
## Pass

Adversary pass 2 on story:run-ingress-admitted-send-outlives-the-drain, against `e91ba82` (room
reserved for the oldest parked send, admission re-read under the queue's lock, a withdrawn send's
sequence given back, room under `drop_stale_epochs` to parked sends first); the cases are
`e843632`, `adversary2_parked_send_queue.rs` and `adversary2_parked_send_run.rs` (test files only).

verdict: CONFIRMED
cases: executed 318→328, red 3
origin: introduced 1 / pre-existing 2 / undecided 0

## Cases

- `a_send_given_up_after_the_receiver_took_its_reserved_envelope_is_not_delivered`: RED, green on
  the base. The window opens when the send is reserved and everything queued before it is taken
  (with capacity 1, the next `recv`) and closes at the send's next poll; threaded, 5 of 206
  given-up sends delivered under `pause` and 3 of 274 under `drop_stale_epochs`, 0 on the base.
  The only sign a caller gets is resending the same envelope on a run ingress, refused
  `SequenceNotIncreasing`.
- `two_reserved_sends_land_in_the_order_they_parked`: RED, red on the base too (needs concurrent
  sends on one sender).
- `two_waiting_ingress_sends_given_up_oldest_first_leave_the_older_sendable_again`: RED, the base
  differently red (needs concurrent sends on one stream).
- Green: a reserved send given up passes its room on and leaks none; `known_contents` stays
  NotEmpty while a send is reserved; at the receiver drop a reserved send answers Ok and counts as
  discarded and a waiting one answers Closed with its envelope; a stale reserved send gives its
  room to the raising send; threaded give-ups under `pause`, `fail` and `drop_stale_epochs` lose
  nothing and hold no room; ingress sends racing a drain from another thread (3 policies × 7
  drain points) are each delivered or answered Closed with their own envelope, and after the
  first refusal every later send is refused. Repeat loops: 6,839 and 15,166 runs, 0 failures.
- Suites: `inference-runtime` 256 passed, 3 failed (the three above); `inference-facade` 69;
  clippy and fmt clean.

## Routing

All three go back to the unit as a second correction, verified by the coordinator's reading of
its diff (no third pass). The introduced one conflicts with pass 1's
`parked_sends_land_in_the_order_they_parked_and_each_is_woken`, which pinned the receiver taking
a reserved envelope before its send is polled again; that case's takes change to poll the woken
send first, its assertions on order and wakes unchanged and its changed lines naming the story.

```findings
[{"file":"crates/inference-runtime/src/queue.rs","line":547,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"the receiver takes a reserved parked envelope before its send is polled again, so a caller that then gives the send up has it delivered with no sign; only a resend of the same envelope on a run ingress tells, by a recorded SequenceNotIncreasing (adversary2_parked_send_queue.rs:196; threaded 5/206 pause, 3/274 drop_stale_epochs, base 0)"},{"file":"crates/inference-runtime/src/queue.rs","line":497,"category":"concurrency","severity":"note","verdict":"INFEASIBLE","origin":"pre-existing","message":"two reserved sends land in the order they are polled, not the order they parked, so one stream arrives 4 before 3 past the sequence guard; needs concurrent sends on one sender, which no caller makes (adversary2_parked_send_queue.rs:257)"},{"file":"crates/inference-runtime/src/drive.rs","line":425,"category":"concurrency","severity":"note","verdict":"INFEASIBLE","origin":"pre-existing","message":"two waiting sends on one stream given up oldest first leave the last sequence at the younger one's previous, so the older envelope sent again is refused though never taken; needs concurrent sends on one stream (adversary2_parked_send_run.rs:437)"}]
```
