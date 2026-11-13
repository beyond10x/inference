---
format: aep.planning-md/3
id: story:inference-agent-close-drain-flake
kind: story
status: draft
title: The device-run close-drain test of inference does not fail under host load
revision: 2
---
## Context

`crates/inference/tests/adversary2_agent.rs:124`
(`frames_fed_while_a_close_drains_do_not_fail_the_device_run`) failed twice on 2026-11-12 in
full workspace runs on a loaded host, during the adversary passes of
story:compare-elevenlabs-cascade-and-ollama, and passed in the next run on the same tree each
time. Found during that work, not caused by it.

## Planned scope

- The test proves the same property without depending on host load: a clock or a signal the
  test controls in place of wall time.

## Acceptance

- The test still fails when the property it proves is broken, shown by a run recorded in the
  change.
- The test passes 50 runs in a row alone and 5 workspace runs in a row.

## Remaining limit (2026-11-13)

The fix feeds frames until the run closes the microphone, so the count of frames that got in
before the feeder stopped no longer matters. One timing dependency stays, as before: with the sink
held, the cascade's playback sink gives up after `PLAYBACK_PATIENCE` (2 s) and the session ends,
which also closes the microphone; frames fed after that never reach the refused send and the test
passes without exercising it. Under an 8-way parallel load the feeder closed the microphone first
in 160 of 160 runs, at most 52 ms after the close. Removing it needs a test-controlled setting for
that limit in production code, which this story did not change.
