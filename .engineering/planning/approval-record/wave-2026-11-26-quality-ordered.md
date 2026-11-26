---
format: aep.planning-md/3
id: approval-record:wave-2026-11-26-quality-ordered
kind: approval-record
status: draft
title: The wave of 2026-11-26 is ordered (D55 A)
relations:
- decides: story:test-run-step-keeps-its-margin
- decides: story:tests-that-fail-on-a-loaded-machine
- decides: story:deepgram-keep-alive-test-flake
- decides: story:drop-tests-own-limit-and-stated-bound
- decides: story:release-check-for-added-ignores
revision: 1
---
## Decision

On 2026-11-26 about 05:18 CET the owner wrote, relayed by the operator session: "now, move in
way bigger /aep:wave and deliver PRs with more stories + gap-closures at once". The coordinator
put the next wave to the operator session as D55 with a default; the operator session answered
"D55 A (U1-U5 on wave/2026-11-26-quality, one PR), taken on the owner's 05:18 CET word."

The wave `wave/2026-11-26-quality` delivers in one pull request:

- U1 story:quality-records-written-by-each-check-run (active before this record).
- U2 story:test-run-step-keeps-its-margin.
- U3 story:tests-that-fail-on-a-loaded-machine, story:deepgram-keep-alive-test-flake and
  story:drop-tests-own-limit-and-stated-bound, one unit because they change the same test files.
- U4 story:release-check-for-added-ignores.

The coordinator ran the moves draft to proposed to active for the five stories of U2 to U4 on
these words. The fifth unit of D55 (the quality pages) is not in the wave: its one finding, the
list of tests of `release-statuses/held`, needs a record from a run on `main` and goes with the
story that adopts such a run.

## What this does not authorise

No call to a hosted provider and no keyed run. No secret, no schedule, no change of a ruleset.
No raised bound of the test step and no shard. No tag and no release follows from this record.
