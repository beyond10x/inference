---
format: aep.planning-md/3
id: story:deepgram-keep-alive-test-flake
kind: story
status: active
title: The Deepgram keep-alive test does not fail under host load
relations:
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T04:28:28Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T04:28:28Z", actor: "human:operator", revision: 3}
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

## Scope as built

Built on the branch `wave/u3-tests-under-load`, merged into `wave/2026-11-26-quality`:
`adapters/inference-deepgram/tests/audio.rs`, `a_keep_alive_is_sent_after_silence`, polls the
server's count of KeepAlives until it reads 2, fails after `KEEP_ALIVE_LIMIT` (5 s) of silence, and
fails when the second KeepAlive takes longer than `KEEP_ALIVE_SPACING` (2 s) at an interval of
100 ms. 50 runs alone: 0 failed.

Left open: the story's five workspace runs in a row were not made; one workspace run passed.
