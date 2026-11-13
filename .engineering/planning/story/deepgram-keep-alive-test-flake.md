---
format: aep.planning-md/3
id: story:deepgram-keep-alive-test-flake
kind: story
status: draft
title: The Deepgram keep-alive test does not fail under host load
revision: 1
---
## Context

`adapters/inference-deepgram/tests/audio.rs:111` (`a_keep_alive_is_sent_after_silence`) failed
on 2026-11-12 in a full `task check` on a loaded host ("1 KeepAlive messages in 600 ms"), during
story:compare-elevenlabs-cascade-and-ollama, which does not touch the crate. Found during that
work, not caused by it.

## Planned scope

- The test proves that a keep-alive follows silence without a wall-clock window that host load
  can stretch.

## Acceptance

- The test still fails when no keep-alive is sent, shown by a run recorded in the change.
- The test passes 50 runs in a row alone and 5 workspace runs in a row.
