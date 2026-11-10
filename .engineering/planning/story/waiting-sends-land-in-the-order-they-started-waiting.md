---
format: aep.planning-md/3
id: story:waiting-sends-land-in-the-order-they-started-waiting
kind: story
status: active
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
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:16:58Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-10T00:16:59Z", actor: "human:operator", revision: 3}
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

Derived 2026-11-10 by `story-scoper`. Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-runtime/src/queue.rs` — cited (Planned scope)
- **Files:** `crates/inference-runtime/src/queue.rs:259-347` (`Shared::poll_send`, the raise at 278-306, the park at 316-320), `:351-364` (`poll_recv`), `:674-680` (`known_contents`), `:655-665` (`QueueReceiver::drop`), `:693-708` (`close_to_senders`), `:153-163` (`Room::poll_room`), `:619-626` (`QueueSender::send`) — cited (read on main af93222a)
- **Symbols:** `Shared::poll_send`, `Shared::poll_recv`, `State::waiting_senders`, `QueueReceiver::known_contents`, `QueueReceiver::close_to_senders`, `Room::poll_room`; the ticket, place and send-future types of `f54cb70` (`Parked`, `Place`, `State::reserve`, `Sending`) lifted in a reduced form — cited
- **Test (new):** `crates/inference-runtime/tests/waiting_send_order.rs` holds `two_reserved_sends_land_in_the_order_they_parked` and `a_send_arriving_after_a_parked_one_is_not_taken_before_it_once_a_raise_made_room`, copied from `f54cb70`, plus truthfulness cases: a granted place that has not landed reads `Empty`, a place the receiver reaches before its send polls is passed, and the receiver's drop refuses a granted, unlanded send as `Closed` with its envelope — inferred
- **Red:** `cargo test -p inference-runtime --test waiting_send_order` (the two copied cases fail on main) — inferred, walked, not run
- **Green:** that command, then `cargo test -p inference-runtime --test held_send_at_drain --test drain_known_contents --test adversary1_review_fixes --test adversary1_wakeups --test queue_close_to_senders --test adversary1_close_to_senders_queue --test bounded_edges`, then `cargo test -p inference-runtime` — inferred
- **Adversary cases compile against main:** yes, by reading (`EdgeQueue`, `QueueSender`, `QueueReceiver`, `InputEndpoint`, `KnownContents`, `OutputEndpoint`, `SendError`, the declaration helpers) — inferred
- **Not ported:** `f54cb70`'s `a_reserved_send_given_up_passes_its_room_on_and_leaks_none`, `a_reserved_send_a_raise_made_stale_gives_its_room_to_the_raising_send` and `reserved_and_waiting_sends_at_the_receiver_drop_are_each_accounted_once`; each asserts that an unlanded place is content, which contradicts this story's acceptance — cited
- **drive.rs:** not needed (`GuardedSender::send` reaches the queue only through `self.inner.send(envelope).await`, `drive.rs:496`) — cited
- **Documents:** the module documentation of `queue.rs:9-24`; a `CHANGELOG.md` line (coordinator) — inferred
- **Confidence:** high for the surface; medium for the design, walked by hand against 9 cases and not built
- **Would collide with:** any unit touching `crates/inference-runtime/src/queue.rs`; nothing in `drive.rs`
- **Safety fact:** an envelope enters `items` only during a poll of its own send that answers `Ready(Ok)`, which keeps a dropped send undelivered (main `queue.rs:307-313`) — unproven

Defect confirmation on main `af93222a` (by reading):
1. A take wakes every waiting send (`queue.rs:354`); whichever polls first pushes (`:307-308`), a fresh send included, and nothing records who waited first.
2. Walked on main, `two_reserved_sends_land_in_the_order_they_parked` receives `[4, 3]` and the `drop_stale_epochs` case `[3, 20, 10]`.
3. Under the design both walk green as `[3, 4]` and `[10, 3, 20]`.
