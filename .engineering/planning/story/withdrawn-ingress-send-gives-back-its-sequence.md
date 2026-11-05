---
format: aep.planning-md/3
id: story:withdrawn-ingress-send-gives-back-its-sequence
kind: story
status: draft
title: A run ingress send that ends untaken gives back its sequence
summary: The sequence guard gives back exactly the sequence of a send whose envelope the queue never took.
tags:
- from-finding
relations:
- decomposes: epic:session-facade
- serves: vision:inference
revision: 1
---
## Context

The adversary passes on story:run-ingress-admitted-send-outlives-the-drain in the 0.14.0 wave
found that a run ingress's sequence guard takes a send's sequence when the send is first polled.
Two cases follow from that:

- A send dropped while it waits for room keeps that sequence. Sending the same envelope again is
  then refused `SequenceNotIncreasing`, though the queue never took it
  (`an_ingress_send_dropped_while_it_waited_can_be_sent_again`, `adversary1_parked_send_run.rs`).
- With two waiting sends on one stream given up oldest first, the older one stays refused
  (`two_waiting_ingress_sends_given_up_oldest_first_leave_the_older_sendable_again`,
  `adversary2_parked_send_run.rs`). That case needs concurrent sends on one stream.

Branch `impl/run-ingress-admitted-send-outlives-the-drain` at `f54cb70` holds a fix: the guard
tracks the highest sequence the queue took and the sequences of sends under way. It also holds
both cases.

## Planned scope

- `crates/inference-runtime/src/drive.rs` (`GuardedSender`).

## Acceptance

A send that ends without the queue taking its envelope gives back exactly its own sequence, in
whatever order such sends end, so the same envelope sent again is taken.
