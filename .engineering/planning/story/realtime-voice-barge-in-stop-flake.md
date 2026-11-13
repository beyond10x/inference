---
format: aep.planning-md/3
id: story:realtime-voice-barge-in-stop-flake
kind: story
status: implemented
title: Barge-in-to-stop test of the voice topology fails intermittently
summary: A wall-clock assertion in adversary2_topology_workers fails in about 2 of 9 runs.
relations:
- serves: vision:inference
- decomposes: epic:elevenlabs-route
- depends_on: story:compare-elevenlabs-cascade-and-ollama
scope:
- confidence: inferred
  path: examples/realtime-voice/src/compare.rs
- confidence: cited
  path: examples/realtime-voice/tests/adversary2_topology_workers.rs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-11-12T15:36:18Z", actor: "human:operator", revision: 6}
- {from: "proposed", to: "active", at: "2026-11-12T15:36:18Z", actor: "human:operator", revision: 7}
- {from: "active", to: "implemented", at: "2026-11-13T00:54:46Z", actor: "human:operator", revision: 8, decided_on: {"recorded":{"test_result":2,"review_outcome":1}}}
---
## Context

`examples/realtime-voice/tests/adversary2_topology_workers.rs:494` asserts a wall-clock bound on
barge-in to stop ("the attack of 30ms needs two frames of 20ms sent at pace"). It failed in 2 of 9
runs on 2026-11-12, once inside the full suite and once in 8 runs alone (measured 3.7 ms and
7.0 ms), review-result:adversary-talk-elevenlabs-pass-1 F7. Found during that review, not caused
by it.

## Planned scope

- The test measures barge-in to stop on a clock the test controls, or bounds it so the suite under
  the shared build pool's load does not fail it, without loosening what it proves. A clock hook
  may go into the cascade runner in `examples/realtime-voice/src/compare.rs`.
- Order: worked after story:compare-elevenlabs-cascade-and-ollama (a `depends_on` edge), which
  changes the same runner in `src/compare.rs` and adds tests under `examples/realtime-voice/tests`.

## Acceptance

- The test still fails when barge-in to stop exceeds the bound it proves today (the attack of
  30 ms needs two frames of 20 ms sent at pace), shown by a run with the stop delayed past that
  bound, recorded in the change.
- The test passes 50 runs in a row alone and 5 full-suite runs in a row.
