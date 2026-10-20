---
format: aep.planning-md/3
id: story:session-interrupt-and-cancel
kind: story
status: implemented
title: Session interrupt and the cancel branch of close
summary: interrupt cuts the active generation at the session boundary and sends cancel into the run; CancelSession closes from Opened, Ready or Draining by cancelling the run.
relations:
- decomposes: epic:session-facade
- serves: vision:inference
- depends_on: story:session-live-text
scope:
- confidence: cited
  path: crates/inference-facade/src/live.rs
- confidence: cited
  path: crates/inference-facade/src/session.rs
- confidence: cited
  path: crates/inference-facade/tests/interrupt_and_cancel.rs
- confidence: cited
  path: crates/inference-testkit/src/session.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/session.rs
- confidence: cited
  path: ess/domains/session.yaml
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:56:33Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T22:56:34Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-20T00:06:55Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

§13.1: interrupt "cancel or supersede active generation/playback" (`docs/design/inference.md:717`);
close "drain or cancel processing and release resources" (719). `ess/domains/session.yaml` leaves
both `UNMAPPED`: interrupt among the facade operations (`session.yaml:158`), and the cancel branch
on `inference.session.CloseSession` (`session.yaml:315-317`). §11.4 (632-641) is what interruption
means below the session: mark the generation superseded, signal cancellation to node instances,
reject stale results at irreversible sinks; cancellation of delivery is distinct from cancellation
of computation (641). The live session and its `Ready` guard on input are
`story:session-live-text`'s.

Decisions taken on silence:

- `inference.session.Interrupt` is a session command refused unless the session is Ready, as
  `SendInput` is; its accepting outcome is `interrupted`. Below the session the facade sends a
  control event of kind `cancel` into every ingress and from then on delivers no egress event
  whose generation id it had delivered before the interrupt. The session is the application's
  irreversible boundary (637), so stale delivery is cut there whatever the nodes still compute.
  What a node does with `cancel` is the node's (a tool round trip: `story:tool-call-interruption`);
  this story changes no node.
- The cancel branch is its own command, `inference.session.CancelSession`, with a lifecycle
  transition `cancel` from Opened, Ready or Draining to Closed, so a drain that does not finish can
  still be cut; its accepting outcome is `cancelled` and it emits `SessionClosed`, since 339 names
  no fifth session event. The facade cancels the run through `RunRequests::cancel` and issues
  `CancelSession` once the run is Cancelled.
- A session that never became ready may be closed either way, as `session.yaml:316` already reads
  it.

## Planned scope

- Spec first, `ess/domains/session.yaml`: `Interrupt` and `CancelSession` with their outcomes,
  refusals and events and the `cancel` transition, `read from:` 717, 719, 637, 339; the marker at
  315 goes, and the one at 158 shrinks to receive and describe. Then `ess specify validate` and
  `ess verify conform synthesize` as AGENTS.md lists.
- `crates/inference-facade`: the two commands in `src/session.rs`; interrupt, generation cut-off
  and the cancelling close in `src/live.rs`.
- Testkit: the session store and its target answer the new commands (`src/session.rs`,
  `tests/target/session.rs`).
- Test `crates/inference-facade/tests/interrupt_and_cancel.rs` (new).

## Domain relations

- Session references Pipeline, one, and Run, zero or one: `ess/domains/session.yaml`,
  `inference.session.Session`, relations `pipeline`, `run`.
- Generation references Run, one: `ess/domains/stream.yaml`, `inference.stream.Generation`,
  relation `run`; the generations the session cuts are its current run's.

## Acceptance

The testkit conform run answers every `inference.session` scenario with none unsupported,
`inference.session.Interrupt/outcome/interrupted`, `inference.session.CancelSession/outcome/cancelled`
and `inference.session.Session/transition/cancel/by/inference.session.CancelSession/cancelled` among
them, and `crates/inference-facade/tests/interrupt_and_cancel.rs` shows, with the mock model
scripted to stream a long reply, that after `interrupt` the session delivers no further event of
that generation while the next send is answered by a new generation, and that cancelling a Ready
session ends with `SessionClosed`, no `SessionDraining`, the run Cancelled and every queue dropped.

## Out of scope

Stopping audio playback on barge-in (A-10, `epic:local-voice-cascade`); metrics separating
cancelled delivery from cancelled computation (641); interrupt over the voice cascade; any change
to a node.
