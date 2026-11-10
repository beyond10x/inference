---
format: aep.planning-md/3
id: story:withdrawn-ingress-send-gives-back-its-sequence
kind: story
status: active
title: A run ingress send that ends untaken gives back its sequence
summary: The sequence guard gives back exactly the sequence of a send whose envelope the queue never took.
tags:
- from-finding
relations:
- decomposes: epic:session-facade
- serves: vision:inference
- decomposes: epic:defects-2026-11-10
scope:
- confidence: cited
  path: crates/inference-runtime/src/drive.rs
- confidence: cited
  path: crates/inference-runtime/tests/adversary1_parked_send_run.rs
- confidence: cited
  path: crates/inference-runtime/tests/adversary2_parked_send_run.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:16:59Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-11-10T00:16:59Z", actor: "human:timo", revision: 5}
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

## Scope

Derived 2026-11-10 by `story-scoper` against `main` at `af93222a`. Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-runtime/src/drive.rs`, `GuardedSender` — cited (story, Planned scope)
- **Files:** `crates/inference-runtime/src/drive.rs:398-516`: the `sequences` field (407-408), its initializer (434), `advance` (446-456), `restore` (459-467), `send` (470-516) — cited
- **Tests, new on main:** `crates/inference-runtime/tests/adversary1_parked_send_run.rs` and `crates/inference-runtime/tests/adversary2_parked_send_run.rs`, taken from `f54cb70` — cited (story names both)
- **Symbols:** `GuardedSender`, `GuardedSender::advance`, `GuardedSender::restore` (becomes `settle`), `Sequences` (new), `Underway` drop guard (new) — cited (`git diff main...f54cb70`)
- **Defect site on main:** `drive.rs:482` takes the sequence at first poll, before `self.inner.send(envelope).await` at `drive.rs:496`; only the refusal path gives it back (`:497-498`), and only while it is still the last one (`:461`) — cited
- **Fix hunks that apply to main:** `Sequences { taken, under_way }` and its `last()`; the field type; `advance` pushing to `under_way`; `settle` replacing `restore`; the `under_way` bookkeeping and `settle(.., sent.is_ok())` in `send` — cited
- **Fix hunks that do not apply:** the module-doc hunk (main rewrote `drive.rs:40-59`); `use crate::queue::{Admits, .., Sending}`; `impl Admits for Signals`; `self.inner.sending(envelope, admits)`; `Sending::withdraw`; `refused_by_admission()` — all need `f54cb70`'s queue.rs — cited
- **Fix hunk to rewrite:** `Underway` wraps main's `self.inner.send(envelope)` (`EndpointFuture`) instead of `Sending`; on drop it gives the sequence back; it keeps main's `!self.inner.is_closed_to_senders()` check (`drive.rs:501`) — inferred
- **queue.rs:** not needed: on main a waiting send keeps its envelope in its own slot and pushes nothing until the poll that answers `Ready` (`queue.rs:257-319`) — inferred
- **Not touched:** `crates/inference-runtime/src/controller.rs` (builds the guard only through `GuardedSender::new`, whose signature stays) — inferred
- **Documents:** a `CHANGELOG.md` `## [Unreleased]` line (coordinator) — inferred
- **Test API on main:** both files should compile unchanged (fixture matches `held_send_at_drain.rs:262-320`) — inferred, not built
- **Red/green:** `cargo test -p inference-runtime --test adversary1_parked_send_run an_ingress_send_dropped_while_it_waited_can_be_sent_again` and `cargo test -p inference-runtime --test adversary2_parked_send_run two_waiting_ingress_sends_given_up_oldest_first_leave_the_older_sendable_again` — cited (commands); outcome inferred
- **Confidence:** high; medium on "compiles unchanged"
- **Would collide with:** any unit touching `crates/inference-runtime/src/drive.rs` (`GuardedSender`, `Signals`). Disjoint from a unit confined to `crates/inference-runtime/src/queue.rs`
- **Safety fact:** on main a `GuardedSender` send future dropped while Pending has never had its envelope pushed (`queue.rs:316-319`; a push answers `Ready` in the same poll, `:307-313`), so giving back the sequence on drop never frees a sequence the queue took — unproven

Not established: whether the other cases in the two files (the subgraph race case at adversary1 line 434; `an_ingress_send_given_up_is_told_delivered_or_not_only_by_sending_it_again` and the threaded drain race in adversary2) pass on main; they are outside this story's acceptance, and adversary2's doc text still names a "reserved envelope" main does not have.

Defect confirmation on `main` at `af93222a` (by reading):
1. `GuardedSender::send` takes the sequence at first poll (`drive.rs:482`), before the queue send is awaited (`:496`).
2. A send dropped while Pending runs no give-back: only an `Err` reaches `restore` (`:497-498`), so resending the same envelope is refused `SequenceNotIncreasing`.
3. `restore` rolls back only while its sequence is still the last (`:461`), to the `previous` it captured, so two sends on one stream given up oldest first leave the older one refused.
