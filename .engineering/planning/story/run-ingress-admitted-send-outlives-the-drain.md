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
revision: 8
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

Taken out of the 0.14.0 wave under its acceptance terms. Each of its three corrections opened a
new defect, and the third could not satisfy one recorded case without changing it.

The work stays on branch `impl/run-ingress-admitted-send-outlives-the-drain`, at head `f54cb70`,
unmerged:

- `df0e382` measured the defect. Under `pause`, a send admitted before the drain into a subgraph
  node's ingress could land while the ingress bridge sent `stop` into the child. It answered `Ok`
  and was discarded when the bridge ended. The run's own ingress delivered that send.
- `f54cb70` carries the last correction and both adversary passes. Its cases are in
  `ingress_admitted_send_at_drain.rs`, `adversary1_parked_send_{queue,run}.rs` and
  `adversary2_parked_send_{queue,run}.rs`.

The next attempt writes the send's state machine down here, and the coordinator accepts it before
any code changes.

### States of one send on a run ingress

| state | entered by | leaves by |
|---|---|---|
| unchecked | `send` called | first poll: refused `Closed` once the run admits no new input, else admitted |
| admitted, room | queue has room | taken under the queue's lock: queued |
| admitted, parked | queue full under `pause` or `drop_stale_epochs` | room reserved, receiver gone (`Closed` with envelope), stale (discarded), send dropped (withdrawn) |
| reserved | a take or a raise gave it a place | its own poll lands it (queued), send dropped (place passed on), receiver gone, stale |
| queued | landed | the receiver takes it (delivered), or the receiver goes (counted discarded) |

### What every design must hold

The adversary passes and the corrections found these requirements:

1. A send given up before its envelope is queued delivers nothing. Passes 1 and 2 found two ways
   to break this: an envelope handed in by the take, and a reserved envelope taken before its
   send's next poll.
2. A send that answers `Ok` reaches the node. The measured defect was a landing after the
   bridge's empty check, discarded when its receiver went.
3. The drain never waits for a caller to poll a send it holds. Correction 2 broke this: the
   bridge waited at a reserved place, so the sequence of a send under a timeout, then
   `drain().await`, then the send awaited, never ended.
4. A send parked on a flat run's ingress before the drain is delivered. This is the recorded
   case `an_admitted_send_waiting_for_room_at_the_drain_reaches_the_node_that_reads_the_ingress`.
5. Pre-existing, needing concurrent sends on one sender (both notes from pass 2):
   - envelopes on one stream reach the node in the order their sends parked;
   - a withdrawal gives back only its own sequence.

### The open choice

Requirements 2, 3 and 4 meet at the bridge. It must decide that its parent edge is empty and
stop further landings in one step.

- Refusing every landing once the ingress is closed meets 2 and 3, but answers `Closed` where
  4 records a delivery.
- Making the bridge's empty check and its close one step under the queue's lock meets all three,
  but the bridge reads its edges only through `InputEndpoint`. That needs a new trait method, a
  public change.

Either way the decision comes before code.
