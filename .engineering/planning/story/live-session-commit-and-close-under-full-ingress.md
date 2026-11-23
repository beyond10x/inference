---
format: aep.planning-md/3
id: story:live-session-commit-and-close-under-full-ingress
kind: story
status: implemented
title: A live session commits and closes over a full ingress only once the run took the commit or the stop
summary: InputCommitted and the turn follow the ingress taking the commit; a close the full ingress refused can be retried.
tags:
- from-finding
relations:
- decomposes: epic:session-facade
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-facade/src/live.rs
- confidence: cited
  path: crates/inference-facade/tests/live_session_close_and_bounded_log.rs
- confidence: cited
  path: crates/inference-facade/tests/text/mod.rs
- confidence: inferred
  path: crates/inference-runtime/src/drive.rs
- confidence: cited
  path: ess/domains/session.yaml
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-23T11:31:38Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-11-23T11:31:38Z", actor: "human:operator", revision: 5, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-11-23T11:31:39Z", actor: "human:operator", revision: 6, decided_on: {"recorded":{"test_result":1}}}
---
## Context

A review of release 0.31.0 drove a live session over a one-place ingress whose overload policy is
`fail` and filled it with one input.

- Finding 8: `commit_input()` answered the queue's `Full`, yet a subscriber received one
  `InputCommitted` and the session's history held the rejected turn "held text". `commit_input`
  runs `CommitInput`, records `InputCommitted` and records the turn before it sends the `commit`
  control event (`crates/inference-facade/src/live.rs:1081-1095`); the ingress refuses that event
  with `Full` under the `fail` policy (`crates/inference-runtime/src/queue.rs:444`). The
  application cannot tell that the turn never reached the run, so it cannot know to retry it.
- Finding 9: `close()` answered `Full` because the ingress refused the `stop` marker. Once the node
  read the queued item there was room, yet a second `close()` was refused (the session is
  Draining, which `CloseSession` does not admit) and the session stayed Draining; only a cancel
  ended it. `close()` lets go of every ingress before it sends the `stop`
  (`live.rs:1109-1125`), and the run counts a source as ended only when its `stop` was taken
  (`crates/inference-runtime/src/queue.rs`, `source_edge_ended`;
  `crates/inference-runtime/src/drive.rs:538`). `CloseError::Forward` documents that the session
  is Draining all the same, with no way to finish the drain.

## Scope

- `crates/inference-facade/src/live.rs`: `commit_input` sends the `commit` into every ingress
  first and runs `CommitInput`, records `InputCommitted` and the turn only once every ingress took
  it; a refused commit leaves the turn open, and a retry sends the `commit` only into the
  ingresses that have not taken it. `close` keeps every ingress that refused its `stop`, and a
  later `close` of a Draining session delivers the `stop` to those. cited
- `ess/domains/session.yaml`: the comment on `CommitInput` states that `InputCommitted` is
  emitted only once the run's ingresses took the commit, as `SendInput` states for `InputSent`. cited
- `crates/inference-facade/tests/live_session_close_and_bounded_log.rs` and
  `crates/inference-facade/tests/text/mod.rs`: the cases below over a one-place `fail` ingress. cited
- `crates/inference-runtime/src/drive.rs`, `controller.rs`: inferred to need no change, since a
  `stop` the ingress takes later ends the source as one taken at once does. inferred

## Acceptance

- Finding 8: with a full one-place `fail` ingress, `commit_input()` answers `Forward` with `Full`;
  no `InputCommitted` is delivered or logged and the history holds no turn. Once the queue has
  room, a second `commit_input()` answers `Ok`, one `InputCommitted` is delivered and the history
  holds the user turn "held text" once.
- Finding 9: with a full one-place `fail` ingress, `close()` answers `Forward` with `Full` and the
  session is Draining. Once the queue has room, a second `close()` answers `Ok`; the run completes,
  `RunCompleted` and then `SessionClosed` are delivered last, and the session is Closed.
- The rustdoc of `close` and `CloseError::Forward` no longer says the session stays Draining; it
  says a later `close()` delivers the end of input.
