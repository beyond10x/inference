---
format: aep.planning-md/3
id: story:synthesiser-drop-inside-runtime
kind: story
status: implemented
title: Dropping an idle synthesis inside a runtime does not block the worker
relations:
- serves: vision:inference
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: adapters/inference-cartesia/src/lib.rs
- confidence: cited
  path: adapters/inference-cartesia/src/synthesiser.rs
- confidence: cited
  path: adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs
- confidence: cited
  path: adapters/inference-cartesia/tests/adversary_w2_drop_process_exit.rs
- confidence: cited
  path: adapters/inference-cartesia/tests/cancel.rs
- confidence: cited
  path: adapters/inference-cartesia/tests/support/deaf.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/lib.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/synthesiser.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/adversary_w2_drop_backed_up.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/adversary_w2_drop_process_exit.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/adversary_w2_drop_where.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/async_handles.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/support/deaf.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/support/dropped.rs
revision: 13
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:08:07Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T13:08:07Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-25T05:11:42Z", actor: "human:operator", revision: 13, decided_on: {"recorded":{"test_result":1,"review_outcome":2,"verification":1}}}
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

## Scope

As built, 2026-11-25. Commits `9fbba75ca`, `69f0e6d88` and `e4d23dcdd`.

- `adapters/inference-elevenlabs/src/synthesiser.rs`, `adapters/inference-cartesia/src/synthesiser.rs`:
  `Drop` asks Tokio whether a runtime is current. Inside one it waits for its close at most
  `CLOSE_GRACE` (5 ms, a new public constant of both crates); a close not written within the
  grace runs to its end on the handle's own thread, bounded by `CLOSE_BOUND` (1 s). Outside a
  Tokio runtime, inside another executor too, the drop waits for its close as before.
- `adapters/inference-elevenlabs/src/lib.rs`, `adapters/inference-cartesia/src/lib.rs`: the crate
  docs say what a drop inside a Tokio runtime does and what the server sees after it.
- `adapters/inference-elevenlabs/tests/async_handles.rs`, `adapters/inference-cartesia/tests/cancel.rs`:
  the case of the Acceptance section and the cases of a close that cannot be written.
- `adapters/inference-elevenlabs/tests/adversary_w2_drop_backed_up.rs` (5 cases),
  `adversary_w2_drop_process_exit.rs` (2), `adversary_w2_drop_where.rs` (4);
  `adapters/inference-cartesia/tests/adversary_w2_drop_backed_up.rs` (4),
  `adversary_w2_drop_process_exit.rs` (1): the cases of the two adversary passes. They bound the
  grace on both sides, hold that a handed-off close is written once when the server reads again,
  and hold the wait of `CLOSE_BOUND` inside an executor that is not Tokio (ElevenLabs only).
- `adapters/inference-elevenlabs/tests/support/deaf.rs`, `support/dropped.rs`,
  `adapters/inference-cartesia/tests/support/deaf.rs`: a server that stops reading. The flood that
  backs the connection up starts at 36,000 pings (9,000 in one case) and is doubled until the
  connection is backed up, at most to 576,000.
- `CHANGELOG.md`: one `Added` and one `Fixed` entry.

Limits, as measured on the build node. The upper bound of the grace case is `CLOSE_GRACE` plus
15 ms; the hold was 5.11 to 11.19 ms over 40 sessions of ten runs, so this assertion is the one a
more loaded host could fail. The doubling past 36,000 pings never ran there. The Cartesia crate
has no case of its own for another executor.

Not built: the wait of Cartesia's discard (up to twice `CLOSE_BOUND`,
`adapters/inference-cartesia/src/synthesiser.rs:486`) predates the story and is not changed; it
is the draft story:cartesia-discard-holds-its-caller-one-bound.
