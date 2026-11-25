---
format: aep.planning-md/3
id: approval-record:quality-time-bound-defaults-ordered-2026-11-25
kind: approval-record
status: draft
title: The tests of the time-bound defaults are ordered (2026-11-25)
relations:
- decides: story:time-bound-defaults-and-close-bounds-held
revision: 1
---
## Decision

On 2026-11-25, after the first quality definition was merged, the owner wrote to the coordinator:
"merge it, then continue also quality checking individual adapters, latency, connect time, etc".

The second quality unit, built on these words, listed what no test holds and filed
story:time-bound-defaults-and-close-bounds-held as a draft. The coordinator asked the operator
session at 09:46 CET what follows the connect-timeout tests: this story, on top of the branch
`quality/connect-timeouts`, local only and with no hosted call (A);
story:connect-time-printed-per-binding (B); or no new unit until the first push of this repository
goes through (C), with A as the default at 10:01 CET.

The operator session answered the same morning: "D35: A (story:time-bound-defaults-and-close-bounds-held
next, local only, no hosted call, no billed run)."

The coordinator ran the moves draft to proposed to active for this story on that.

## The coordinator's cut of the story

The scope read of the same day found two parts of different size. 34 defaults have no test that
fixes their value, and a comparison with a literal number in an existing test file holds each. The
close bounds need a peer that stops reading in each fixture, and the read could not determine
whether every bound can be reached. The coordinator narrowed this story to the defaults, the value
of the five `CLOSE_BOUND` constants among them, and filed the firing of the close bounds as the
draft story:close-bounds-fired-by-a-test. The story's Context, Planned scope, Acceptance and Out of
scope were rewritten on that. The title of this story still names both parts.

## What this does not authorise

No live test, no example and no benchmark is run: a call to a hosted provider is billed, and that
needs the owner's word of its own. No bound, default or error of an adapter changes. Nothing is
pushed while the push of this repository is refused; the branch stays local. No tag and no release
follows from this record. The draft story:close-bounds-fired-by-a-test is not accepted by this
record.
