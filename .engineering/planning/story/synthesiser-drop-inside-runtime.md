---
format: aep.planning-md/3
id: story:synthesiser-drop-inside-runtime
kind: story
status: draft
title: Dropping an idle synthesis inside a runtime does not block the worker
revision: 1
---
## Context

Dropping an idle synthesis handle with a connection open blocks the dropping thread while the
close completes, up to `CLOSE_BOUND` (1 s): `adapters/inference-elevenlabs/src/synthesiser.rs`
`Drop`, and the same in Cartesia's. On an async worker that stalls every task on the thread. The
agent sessions document that a drop inside a runtime does not wait; the synthesisers neither
document nor avoid it. Found by adversary pass 1 on story:async-speech-handles; it predates that
story (the base `Drop` blocked the same way through `run`).

## Planned scope

- The ElevenLabs and Cartesia synthesiser `Drop` does not block when a runtime is current: the
  close runs on the synthesis's own runtime thread or is abandoned, as the agent sessions do.
- The crate docs say what a drop inside a runtime does.

## Acceptance

- Dropping an idle synthesis with an open connection on a current-thread runtime, with a 10 ms
  ticker task running, keeps the ticker's largest gap under 50 ms.
