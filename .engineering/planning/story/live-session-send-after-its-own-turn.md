---
format: aep.planning-md/3
id: story:live-session-send-after-its-own-turn
kind: story
status: active
title: A session's send of an input the run took is never refused by the close that input caused
tags:
- from-finding
relations:
- serves: vision:inference
- decomposes: epic:defects-2026-11-10
scope:
- confidence: cited
  path: crates/inference-facade/src/live.rs
- confidence: inferred
  path: crates/inference-facade/tests/send_taken_then_drained.rs
- confidence: cited
  path: crates/inference/tests/agent.rs
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:16:58Z", actor: "human:operator", revision: 6}
- {from: "proposed", to: "active", at: "2026-11-10T00:16:58Z", actor: "human:operator", revision: 7}
---
## Context

Found on 2026-11-08 while the fourth SDK wave made its agent cases deterministic under load; it
lives in `inference-facade` and predates that wave.

`LiveSession::send` (`crates/inference-facade/src/live.rs:1023-1030`) hands the envelope to the
run first and records the send afterwards. When that envelope completes a turn whose reply makes
the model close the session (the duplex mock `.closes()` once its script is spent), the session
can already be Draining when the send is recorded, and the send answers `InputError::Refused`
("the session is Draining") for an input the run took. Its documentation allows a refusal only
before the input was taken. Seen in about 6 of 1,000 lanes of
`cargo test -p inference --features testing --test agent` with six lanes at once (load 14-33),
each in a different test, through the test helper `speak()`; the mock had taken every frame.

## Acceptance

- An input the run took is answered `Ok`, whatever the session's state when the send is recorded;
  a send refused before the run took it answers `Refused` with its envelope, as now.
- A test that closes the session from the model at the input that completes the turn sees `Ok`
  for that send in every one of 1,000 rounds at 16 sessions at once.

## Scope

Rewritten 2026-11-10 from the merged change (`git diff ff0ba010 45748589`, 6 files).

- **Source:** `crates/inference-facade/src/live.rs`: `send` answers `Ok` with no `InputSent` for an input the run took while the session drained; the new `InputError::Cancelled`, answered before the forward when the session was cancelled and after it when a cancel overtook the send, so `Refused` keeps its promise that nothing was forwarded
- **Caller:** `crates/inference/src/agent.rs` (`input_error` maps `Cancelled` to `Error::Closed`)
- **Tests:** `crates/inference-facade/tests/send_taken_then_drained.rs` (red on the base at `:160` and `:188`), `crates/inference/tests/agent.rs`, the adversary files `adversary_u3_send_after_drain.rs` and `adversary_u3_send_after_drain_pass2.rs`
- **Reviews:** review-result:adversary-u3-send-after-drain-pass-1, review-result:adversary-u3-send-after-drain-pass-2

## Decision

The coordinator took these decisions on 2026-11-10, before code, from the scoper's two findings:

- "Refused with its envelope, as now" is read as "Refused, as now". `InputError::Refused` carries a
  `SessionMoveError` and no envelope (`crates/inference-facade/src/live.rs:629-631`).
- "Whatever the session's state" means the following. Once the ingress took the envelope, `send`
  answers `Ok` if the session moved to Draining while the send was in flight, or on to Closed by
  that drain. The session's records tell whether a later state came from a drain or from a cancel;
  the state alone does not.
- `InputSent` is recorded only where `SendInput` admits it, which is Ready. An input taken after the
  session left Ready answers `Ok` without an `InputSent`, and the doc comment of `send` says so.

The second adversary pass (review-result:adversary-u3-send-after-drain-pass-2) showed that a
cancelled session answering `Refused` broke `Refused`'s promise that nothing was forwarded. The
coordinator then changed the cancel case and the `InputError` type:

- A send in a cancelled session answers the new `InputError::Cancelled`. That covers a send started
  after `cancel()`, which forwards nothing, and a send that a cancel overtook while the ingress took
  it, whose envelope the cancelled run discards.
- `Refused` again means that nothing was forwarded.
