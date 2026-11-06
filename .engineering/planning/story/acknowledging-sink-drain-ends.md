---
format: aep.planning-md/3
id: story:acknowledging-sink-drain-ends
kind: story
status: draft
title: The acknowledging sink's drain ends when the device stops, and keeps receipts of audio already heard
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
scope:
- confidence: inferred
  path: adapters/inference-audio-device/src/acknowledging.rs
- confidence: inferred
  path: adapters/inference-audio-device/src/playout.rs
- confidence: inferred
  path: adapters/inference-audio-device/tests/acknowledging.rs
revision: 4
---
## Context

From review-result:full-codebase-2026-11-06 (findings W6, N30, N31, N36), at release 0.16.0, all in `adapters/inference-audio-device`:

- W6 `src/acknowledging.rs:549-556`, `:665-673`, `:882-887`: `drain()` waits with no deadline on a played position only the device callback advances; the sink never reads the playout's recorded stream errors.
- N30 `acknowledging.rs:446`, `:465-476`: `turn_opened` calls `forget_held()` even when nothing is left to play, dropping the receipt of a fully heard segment.
- N31 `acknowledging.rs:549-553` with `src/playout.rs:180-191`: a frame starting a new clock below an earlier held end makes every later receipt wait forever; the one-clock assumption is documented, not checked.
- N36 `tests/acknowledging.rs:717-733`: `played_after` is always `None` after `speaker.stop()`, so the assertion is `None == None`.

## Planned scope

- `drain()` fails with a typed error when the playout records a stream error, and has a stated patience while the played position does not move.
- `turn_opened` keeps receipts of segments already fully played.
- Refuse, with a typed error, a frame whose clock goes back below a held end.
- Rewrite the N36 assertion so it can fail.

## Acceptance

- A speaker whose callback stops makes `drain()` fail within the stated patience.
- A recorded stream error fails `drain()` at once.
- A `TurnOpened` after the last frame played and before the receipt still acknowledges that segment.
- A clock regression is refused instead of hanging.
- The rewritten N36 assertion fails against a mutant.
