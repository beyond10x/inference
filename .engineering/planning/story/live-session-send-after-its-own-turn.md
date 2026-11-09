---
format: aep.planning-md/3
id: story:live-session-send-after-its-own-turn
kind: story
status: draft
title: A session's send of an input the run took is never refused by the close that input caused
tags:
- from-finding
relations:
- serves: vision:inference
revision: 1
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
