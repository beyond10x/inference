---
format: aep.planning-md/3
id: story:realtime-voice-barge-in-stop-flake
kind: story
status: draft
title: Barge-in-to-stop test of the voice topology fails intermittently
summary: A wall-clock assertion in adversary2_topology_workers fails in about 2 of 9 runs.
relations:
- serves: vision:inference
revision: 1
---
## Context

`examples/realtime-voice/tests/adversary2_topology_workers.rs:494` asserts a wall-clock bound on
barge-in to stop ("the attack of 30ms needs two frames of 20ms sent at pace"). It failed in 2 of 9
runs on 2026-11-12, once inside the full suite and once in 8 runs alone (measured 3.7 ms and
7.0 ms), review-result:adversary-talk-elevenlabs-pass-1 F7. Found during that review, not caused
by it.

## Planned scope

- The test measures barge-in to stop on a clock the test controls, or bounds it so the suite under
  the shared build pool's load does not fail it, without loosening what it proves.

## Acceptance

- The test passes 50 runs in a row alone and 5 full-suite runs in a row.
