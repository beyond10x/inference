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
revision: 3
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
