---
format: aep.planning-md/3
id: story:nova-sonic-tests-wait-for-their-event
kind: story
status: draft
title: The Nova Sonic duplex tests wait for the event they assert, not for 100 ms of quiet
relations:
- serves: vision:inference
revision: 1
---
## Context

`adapters/inference-aws/tests/nova_sonic_duplex.rs` takes events with `drain`, which stops after
100 ms with no event. Two of its tests failed on a loaded host because a replay gap longer than
that ended the wait before the event they assert came; unit U3 of the wave of 2026-11-26
(story:tests-that-fail-on-a-loaded-machine) moved those two to `until`, which waits for the event
it needs. The implementor of that unit counted 11 more call sites in the same file that wait for a
quiet time where an event exists. The other test files of `inference-aws` have `drain` helpers of
their own, not checked.

## Acceptance

- Every call site in the tests of `inference-aws` that asserts an event after a `drain` waits for
  that event (or for the end of the session) instead of for 100 ms of quiet; a list of the call
  sites with file and line, and for each the event it now waits for, is in the change.
- With a replay gap of 150 ms planted in the replay of each changed file, every test of that file
  passes; without the change, the tests whose event comes after such a gap fail.
- A call site that asserts that nothing more comes keeps a quiet wait and says so in one line.

## Out of scope

- The adapter's code.
