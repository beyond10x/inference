---
format: aep.planning-md/3
id: approval-record:quality-close-bounds-ordered-2026-11-25
kind: approval-record
status: draft
title: The tests that fire a close bound are ordered (2026-11-25)
relations:
- decides: story:close-bounds-fired-by-a-test
revision: 1
---
## Decision

On 2026-11-25, after the first quality definition was merged, the owner wrote to the coordinator:
"merge it, then continue also quality checking individual adapters, latency, connect time, etc".

The fourth quality unit, built on these words, fixed the value of the time-bound defaults and
filed story:close-bounds-fired-by-a-test as a draft. The coordinator asked the operator session at
11:31 CET what follows: this story, local only, with fixtures under `tests/` changed and no change
under `src/` (A); story:connect-time-printed-per-binding (B); or no new unit until the first push
of this repository goes through (C), with A as the default at 11:46 CET.

The operator session answered before that time: "D38 = A (story:close-bounds-fired-by-a-test,
local only, no hosted call, no src change)".

The coordinator ran the moves draft to proposed to active for this story on that.

## What this does not authorise

No live test, no example and no benchmark is run: a call to a hosted provider is billed, and that
needs the owner's word of its own. No bound, default or error of an adapter changes, and no file
under `adapters/*/src` changes. Nothing is pushed while the push of this repository is refused;
the branch stays local. No tag and no release follows from this record.
