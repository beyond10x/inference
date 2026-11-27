---
format: aep.planning-md/3
id: approval-record:quality-connect-timeouts-ordered-2026-11-25
kind: approval-record
status: draft
title: The connect-timeout tests of five bindings are ordered (2026-11-25)
relations:
- decides: story:connect-timeouts-fired-by-a-test
revision: 1
---
## Decision

On 2026-11-25, after the first quality definition was merged, the owner wrote to the coordinator:
"merge it, then continue also quality checking individual adapters, latency, connect time, etc".

The second quality unit, built on these words, listed what no test holds and filed
story:connect-timeouts-fired-by-a-test as a draft. The coordinator asked the operator session at
08:34 CET whether the draft is accepted and built now (A), built together with
story:time-bound-defaults-and-close-bounds-held (B), or held until the owner answers the open
rulings (C), with A as the default at 08:49 CET.

No answer had arrived at 08:49 CET. Under the rule "A default proceeds" of the organization's
rules for agent sessions the coordinator took A on the default (D32 taken on default). The step
is reversible: a local branch, nothing pushed.

The coordinator ran the moves draft to proposed to active for this story on that. The story's
Context, Planned scope, Acceptance and Out of scope were rewritten from the scope read of the same
day: five bindings instead of three, and the page entries and records follow after a `check` run
on `main`.

## What this does not authorise

No live test, no example and no benchmark is run: a call to a hosted provider is billed, and that
needs the owner's word of its own. No bound, default or error of an adapter changes. Nothing is
pushed while the push of this repository is refused; the branch stays local. No tag and no release
follows from this record.
