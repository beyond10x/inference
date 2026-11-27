---
format: aep.planning-md/3
id: approval-record:wave-2026-11-26-adoption-ordered
kind: approval-record
status: draft
title: The wave of 2026-11-26 that adopts the records of a run on main, with two test stories
relations:
- decides: story:quality-records-adopted-from-a-main-run
- decides: story:ticker-test-holds-under-contention
- decides: story:nova-sonic-tests-wait-for-their-event
revision: 1
---
## Decision

On 2026-11-26 the coordinator put the next wave to the operator session as D61 with options and a
default. The operator session answered on the owner's delegation: "D61 A — all three stories in
the next wave, one PR. The two flaky-test stories fix the tests (event waits instead of quiet
timers, contention-proof), not production code." It stands on the owner's word of 05:18 CET the
same day to deliver bigger waves with more stories in one pull request.

The wave `wave/2026-11-26-adoption` delivers in one pull request:

- story:quality-records-adopted-from-a-main-run, the records of the check run 45771353687 on
  `main` in place of the records written by hand, and the pages that name its tests.
- story:ticker-test-holds-under-contention, a finding of the wave of 2026-11-26 that predates it.
- story:nova-sonic-tests-wait-for-their-event, a finding of the same wave that predates it.

The coordinator ran the moves draft to proposed to active for the three stories on these words.

## What this does not authorise

No call to a hosted provider and no keyed run. No secret, no schedule, no change of a ruleset. No
series of records (the owner's open decision on where it lives). No raised bound of the test step
and no shard. No tag and no release follows from this record.
