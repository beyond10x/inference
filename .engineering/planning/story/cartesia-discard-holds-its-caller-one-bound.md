---
format: aep.planning-md/3
id: story:cartesia-discard-holds-its-caller-one-bound
kind: story
status: draft
title: Cartesia discard holds its caller one bound and no runtime thread
tags:
- from-finding
relations:
- informed_by: review-result:synthesiser-drop-inside-runtime-adversary-1
- serves: vision:inference
revision: 1
---
## Context

The adversary pass on the synthesiser drop (review-result:synthesiser-drop-inside-runtime-adversary-1,
finding 5) measured `discard` of the segment being spoken on the Cartesia synthesiser, inside a
runtime, with a connection whose send side was backed up: the call held the runtime's thread
2.0016 s at the unit's head and 2.0012 s at its base, with a ticker gap of 2.01 s. The cancel
write waits `CLOSE_BOUND`, then `close_now` waits another
(`adapters/inference-cartesia/src/synthesiser.rs:479`, `:490`). The docs at `:696` and
`src/lib.rs:27` name one `CLOSE_BOUND`. The backed-up state was built by the case (a server that
stops reading behind a flood of pings); no provider was shown to do it. The behaviour predates the
unit, which changed the drop and not `discard`.

## Planned scope

- `discard` holds its caller at most the bound its docs name, and inside a runtime it does not
  hold the runtime's thread for it: the same rule the drop got.
- The docs and the code name the same bound.

## Acceptance

- With a connection whose send side is backed up, `discard` of the segment being spoken inside a
  current-thread runtime keeps a 10 ms ticker's largest gap under 50 ms (state transition: the
  segment is discarded).
- Outside a runtime `discard` returns within one `CLOSE_BOUND`.
