---
format: aep.planning-md/3
id: story:waiting-sends-land-in-the-order-they-started-waiting
kind: story
status: implemented
title: Waiting sends on one queue land in the order they started waiting
summary: Order the sends that wait for room under pause and drop_stale_epochs without making known_contents untruthful.
tags:
- from-finding
relations:
- decomposes: epic:session-facade
- serves: vision:inference
- decomposes: epic:defects-2026-11-10
scope:
- confidence: cited
  path: crates/inference-runtime/src/queue.rs
- confidence: inferred
  path: crates/inference-runtime/tests/waiting_send_order.rs
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:16:58Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-10T00:16:59Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-10T02:46:22Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":1}}}
---
## Context

The adversary passes on story:run-ingress-admitted-send-outlives-the-drain in the 0.14.0 wave
found, from the code and with cases, two ways a queue under `pause` or `drop_stale_epochs`
lets waiting sends overtake one another:

- room a take makes goes to whichever waiting send is polled first;
- under `drop_stale_epochs`, a send that arrives after a waiting one can be taken before it once
  a raise made room.

With several sends in flight on one sender, envelopes of one stream can reach the node out of
order, past the sequence guard. No caller in the workspace makes concurrent sends on one sender.

The cases are on branch `impl/run-ingress-admitted-send-outlives-the-drain` at `f54cb70`:

- `two_reserved_sends_land_in_the_order_they_parked` (`adversary2_parked_send_queue.rs`);
- `a_send_arriving_after_a_parked_one_is_not_taken_before_it_once_a_raise_made_room`
  (`adversary1_parked_send_queue.rs`).

That branch also holds one fix, the reserved-place queue, which the 0.15.0 wave dropped: a
reserved place answered `known_contents` `NotEmpty` before its send landed, and a reader draining
by that answer waited on the send's caller.

## Planned scope

- `crates/inference-runtime/src/queue.rs`.
- A design first that keeps `known_contents` truthful, holds the drain from waiting on a caller,
  and orders waiting sends.

## Acceptance

Waiting sends on one queue land in the order they started waiting, under `pause` and
`drop_stale_epochs`. `known_contents` answers `NotEmpty` only when a `recv` hands an envelope
over, and the held-send cases of `held_send_at_drain.rs` stay green.

## Design

Proposed by the scoper on 2026-11-10 and taken by the coordinator before code:

1. A send that finds no free room takes a ticket under the lock: a sequence number, the producer, the epoch and a waker; its envelope stays with the send. Dropping the send withdraws the ticket and passes on any room it held.
2. Whenever room appears (a take, a raise's purge, a withdrawal, a pass-over), it goes under the lock to the oldest non-stale ticket without a place, as a `Place::Granted(ticket)` at the back of `items`. Every waiting send is woken, as on main.
3. A new send takes room only if no ticket is still waiting without a place; otherwise it takes a ticket. A later send cannot overtake an older waiting one.
4. A send that holds a place lands only by its own poll, into its place.
5. `recv`, `len` and `known_contents` see only landed envelopes, so `NotEmpty` means an envelope has landed.
6. `recv` takes the first landed envelope; every place in front of it is passed over and its ticket keeps its age. With nothing landed, `recv` waits, or answers `None` when the queue is closed or has no senders. The receiver never waits on a place, so a drain never waits on a caller.
7. `close_to_senders` and the receiver's drop remove every place and ticket; each of those sends is refused `Closed` with its envelope at its next poll, and nothing unlanded is counted as discarded.
8. A stale ticket under `drop_stale_epochs` leaves the line at the raise and completes as discarded at its next poll, as on main.
9. `Room::poll_room` counts places as occupied.

Decision on the one residual (coordinator, 2026-11-10): a send whose place the receiver reaches
before that send polls again loses the place, not its envelope, and lands after younger envelopes.
A strict order under every polling schedule would make the receiver wait for that caller, which
the drain rule (`held_send_at_drain.rs`) forbids. The acceptance is read as: waiting sends land in
the order they started waiting whenever each is polled before the receiver reaches its place; a
test pins the residual as today's behaviour.

## Scope

Rewritten 2026-11-10 from the merged change, `git diff ff0ba010 ae789d7a`.

- **Source:** `crates/inference-runtime/src/queue.rs`.
  - Each waiting send gets a ticket, and room is granted in ticket order.
  - A granted place lands only through its own send's poll. Given up, it passes to the next
    ticket.
  - `len` and `known_contents` count only envelopes that landed.
  - A drain does not wait on a caller.
  - `drive.rs` is untouched.
- **Tests:**
  - `crates/inference-runtime/tests/waiting_send_order.rs`: 3 of its 5 cases are red on the base.
  - The adversary files `adversary_u7_waiting_sends.rs` and `adversary_u7_threads.rs`.
- **Residual, accepted in the design:** a send whose place the receiver reached before the send
  polled lands after younger envelopes.
- **Kept after the review:** two cases stay as they are.
  - A granted place whose send stays alive but unpolled holds the room.
  - The stale branch of `poll_ticket` repeats a removal that the epoch raise already made.
- **Reviews:** review-result:adversary-u7-waiting-sends-pass-1.
