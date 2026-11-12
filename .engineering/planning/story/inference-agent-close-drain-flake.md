---
format: aep.planning-md/3
id: story:inference-agent-close-drain-flake
kind: story
status: draft
title: The device-run close-drain test of inference does not fail under host load
revision: 1
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
