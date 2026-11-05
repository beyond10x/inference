---
format: aep.planning-md/3
id: story:run-ingress-admitted-send-outlives-the-drain
kind: story
status: active
title: A send a run ingress admitted before its drain is delivered or reported
summary: Measure whether a send admitted under the pause policy can still wait when the drain ends the bridge, and close the gap if it can.
tags:
- from-finding
relations:
- decomposes: epic:session-facade
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-runtime/src/drive.rs
- confidence: cited
  path: crates/inference-runtime/src/queue.rs
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-04T07:29:54Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-04T07:29:54Z", actor: "human:operator", revision: 3}
---
## Context

Raised by the adversary pass on story:subgraph-nested-drain-residual-loss in the 0.13.0 wave
(2026-11-04), `review-result:adversary-subgraph-nested-drain-pass-1`, as undecided and read from
the code only. The 0.13.0 fix of the ingress bridge rests on "the parent drains the node only once
everything it admitted is on the node's edges". At a run's own ingress
(`crates/inference-runtime/src/drive.rs`, `GuardedSender`, about line 437) the admission
(`admits_input`) is checked on the first poll and the send then awaits room under the pause
policy, so a send admitted before the drain could still be waiting when the bridge finds the
edge empty and ends. That send would answer `Closed`, or its envelope be discarded with the
queue. Not reproduced.

## Planned scope

- Measure first: a deterministic test that fills a run ingress under the pause policy, admits a
  send, drains, and records whether the admitted envelope reaches the node or what the send
  returns.
- If it is lost silently: make admission and the push one step, or make the drain wait for every
  admitted send.

## Acceptance

A send a run ingress admitted before its drain either reaches the node or returns an error that
says it was not delivered; a test holds whichever the runtime does.

## Design

The 0.14.0 wave took this story out under its acceptance terms, after two adversary passes and three
corrections. The work stays unmerged on branch `impl/run-ingress-admitted-send-outlives-the-drain`
at `f54cb70`. The coordinator accepts what follows before any code changes.

### The defect

Under `pause`, a send admitted before a drain into a subgraph node's run ingress waits for room.
The node's ingress bridge drains: it finds its parent edge empty, sends `stop` into the child and
ends. In the meantime the waiting send lands on the edge, answers `Ok`, and is discarded when the
edge's receiver goes. The run's own ingress, read by a plain node, delivers the same send.
Measured by `df0e382`: the subgraph case was red on the base, `Ok` with the child receiving one,
two, three, stop.

### What every design must hold

1. A send given up before its envelope is in the queue delivers nothing.
2. A send that answers `Ok` reaches the node that reads the edge.
3. The drain never waits for a caller to poll a send it holds. A caller that polls a send under a
   timeout, then awaits the run's drain, then awaits the send, gets its drain back.
4. On a plain node's run ingress, a send parked before the drain is delivered
   (`an_admitted_send_waiting_for_room_at_the_drain_reaches_the_node_that_reads_the_ingress`).
5. Needing concurrent sends on one sender (pass 2, pre-existing): envelopes on one stream reach
   the node in the order their sends parked, and a withdrawal gives back only its own sequence.

### The choice: the bridge closes its edge to senders in one step

The other choice, refusing every landing once the ingress is closed, breaks 4. This design keeps
the queue of `f54cb70`:

- room is reserved as a place in queue order;
- an envelope lands only by its own send's poll;
- the ingress guard tracks the sequences taken and those under way.

It adds one step that the bridge takes at its drain.

`inference_core::endpoint` gains an additive provided method and its answer:

```rust
/// Closes the edge to its senders and, in the same step, says what it still holds.
/// After it, every send into the edge, and every send still waiting for room or holding
/// a reserved place, is refused as `SendError::Closed` with its envelope.
/// `recv` hands over what the edge held and then answers `None`.
/// The default answers `Unsupported` and closes nothing, so an endpoint that keeps it
/// behaves as before, and a caller given that answer decides as it would without
/// this method.
fn close_to_senders(&mut self) -> ClosedToSenders {
    ClosedToSenders::Unsupported
}

pub enum ClosedToSenders {
    /// Closed, holding nothing.
    Empty,
    /// Closed, holding at least one envelope.
    Holding,
    /// Not closed.
    Unsupported,
}
```

`QueueReceiver` overrides it. Under the queue's lock:

- it marks the queue closed to senders;
- it removes every reserved place whose send has not landed;
- it answers from the envelopes that did land.

A send that has not landed is refused with its envelope at its next poll, and a new send is
refused at once. The receiver stays, so the landed envelopes are still taken. `recv` never waits
on a closed queue: it takes the next landed envelope, or answers `None`.

`SinkReceiver` and every other endpoint keep the default.

### Who calls it, and when

| caller | at | does |
|---|---|---|
| the subgraph ingress bridge (`ingress_bridge`) | its first read of the drain, once per parent edge | `Empty` ends that edge; `Holding` receives until `None`; `Unsupported` keeps the 0.14.0 path (`known_contents`, the bounded re-polls) |
| a plain node | never | its ingress keeps 4 |

A refusal by a queue closed to senders reaches the ingress guard as `Closed`. The guard reports
it as a send that was not delivered, and it does not mark the consumer node gone: the node is
draining, not gone. A withdrawn or refused send gives back its sequence, as in `f54cb70`.

### The states of one send on a run ingress

| state | entered by | leaves by |
|---|---|---|
| unchecked | `send` called | first poll: refused `Closed` once the run admits no new input; else admitted |
| admitted, room | the queue has room | taken under the lock: queued |
| parked | the queue is full under `pause` or `drop_stale_epochs` | a reserved place; or refused `Closed` with its envelope (closed to senders, or receiver gone); or discarded (stale); or withdrawn (send dropped) |
| reserved | a take or a raise gave it a place | its own poll lands it: queued; or refused `Closed` with its envelope (closed to senders, or receiver gone); or withdrawn, passing the place on; or discarded (stale) |
| queued | landed | taken by the receiver: delivered; or the receiver goes: counted discarded |

"Closed to senders" acts on parked and reserved sends only. It never touches a queued envelope,
so 2 holds. The bridge never waits on a send that has not landed, so 3 holds. A plain node never
closes its edge, so 4 holds. 1 and 5 hold as in `f54cb70`.

### Acceptance, on tests

1. Every case at `f54cb70` stays green, unchanged:
   - `ingress_admitted_send_at_drain.rs`;
   - `adversary1_parked_send_queue.rs` and `adversary1_parked_send_run.rs`;
   - `adversary2_parked_send_queue.rs` and `adversary2_parked_send_run.rs`.

   The subgraph case that awaits the send once the run has ended comes back, and answers `Closed`
   with its envelope.
2. A held-send case in caller shape: the send polled under a timeout that fires while it waits,
   then the run's drain awaited, then the send awaited, each await bounded. It is red on
   `f54cb70` (the drain times out). Expected green on `main` (to be measured), where the send answers `Closed` after
   the drain ends, and green on the unit.
3. `QueueReceiver::close_to_senders` races a landing send from another thread. Every send is
   either taken by the bridge or refused `Closed` with its envelope, never `Ok` and discarded.
   The threaded give-ups under `pause` and `drop_stale_epochs` deliver no given-up send.
4. One adversary pass, at most one correction. A red case, or an earlier case reopened, takes the
   unit out again.
