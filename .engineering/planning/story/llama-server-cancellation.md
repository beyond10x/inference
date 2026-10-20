---
format: aep.planning-md/3
id: story:llama-server-cancellation
kind: story
status: draft
title: 'llama-server adapter: cancellation, failures, deadlines and disconnects'
summary: Delivery and computation cancellation reported apart; 503, disconnect and idle deadline cancel the generation with the llm error kept; no leaked slot.
relations:
- decomposes: epic:local-llm-adapter
- serves: vision:inference
- depends_on: story:llama-server-adapter
revision: 3
---
## Context

The exit of `epic:local-llm-adapter` asks for section 23.2 lines 1114-1116 against the llama-server
adapter: capability declarations match observed behaviour; errors, deadlines, cancellation and
finalisation propagate; backend disconnects leak no task and revive no stale output. Cancellation
of delivery is distinct from cancellation of computation, and both are reported
(`docs/design/inference.md:641`).

What exists, read at `llm` tag 0.8.0 and in this repository:

- `GenerationHandle::cancel` runs `inference.stream.CancelGeneration`, cancels the running turn
  through `llm_core::Cancel` and delivers nothing more (`crates/inference-llm/src/lib.rs:1001-1014`).
  `GenerationCancelled` is delivery cancellation: "computation may still finish"
  (`ess/domains/stream.yaml:728`).
- A turn that fails cancels the generation, delivers `GenerationCancelled` with the segments it
  discarded and keeps the `llm` error (crate documentation of `inference-llm`).
- `llm-http` ends a cancelled stream with `Error::cancelled()` and `Dispatch::Accepted` and drops
  the response, which closes the connection (`crates/llm-http/src/transport.rs:290-309`); its
  `Limits` bound the wait for headers, the idle gap and the total (`transport.rs:39-46`, defaults
  60 s, 180 s, 600 s).
- `llm` reports nothing about whether the server stopped computing. That belongs to the backend,
  and the adapter observes it only live.

## Domain relations

- `inference.stream.Generation -> inference.stream.OutputSegment`: one-to-many, the generation
  owns its segments, and cancelling it discards the unplayed ones. Inferable from
  `ess/domains/stream.yaml`, entity `inference.stream.Generation`, relation `segments` (owns).

## Planned scope

- The HTTP limits are configuration of `story:llama-server-adapter`; this story tests them.
- Offline tests in `adapters/inference-llama-server` against the loopback fixture server:
  - cancel mid-stream: after the first `EmitTextDelta` the test cancels; `GenerationCancelled` is
    delivered, no delta follows, and the fixture observes its connection closed before it sent
    the rest of the stream. The closed connection is the evidence that the adapter asked the
    backend to stop computing;
  - failure before output: the fixture answers HTTP 503 with a model-loading body; the generation
    is cancelled with no segment and `failure()` keeps the `llm` error code;
  - disconnect mid-stream: the fixture closes after one delta with no finish reason and no
    `[DONE]`; the generation is cancelled with that segment among the discarded, the error kept,
    and no later event;
  - deadline: with a short idle limit the fixture stalls after one delta; the generation is
    cancelled with the `llm` timeout error kept;
  - no leak: after each case above, with maximum concurrency 1, a new generation starts on the
    same `Generator` (its slot was given back), and no task of the turn keeps the handle alive
    (a weak reference, as `crates/inference-runtime/tests/text_pipeline.rs` checks its exchange
    tasks).
- The capability record of `story:llama-server-adapter` holds evidence per claimed capability, and
  this story claims none (`interruptible` stays unclaimed). So the two cancellations are reported
  by separate tests: delivery cancellation is `GenerationCancelled` with no later delta; the
  request to stop computation is the closed connection offline and the slot state of the server
  live, asserted or printed by the test that observes it.
- A live test, `#[ignore]` and behind feature `live`, reading `INFERENCE_LLAMA_SERVER_URL` and
  `INFERENCE_LLAMA_SERVER_MODEL` and returning at once when either is unset: it starts a long
  generation, cancels after the first delta, asserts `GenerationCancelled` and no later delta, then
  reads the slot state of the server (`GET /slots` at the server root) until no slot is processing
  or 10 s pass, and prints which happened. A server that does not expose slot state is reported as
  "computation cancellation not observed", not as a failure.

## Acceptance

`cargo test -p inference-llama-server --all-features` passes offline with the mid-stream cancel,
503, disconnect, idle-deadline and slot-release tests named above; delivery cancellation and the
request to stop computation are asserted by separate tests.

## Out of scope

Retries (routing in `llm`, see `story:llm-0-8-0`); supervising or restarting the server; claiming
`interruptible`; tool round trips (`story:llama-server-tool-proposals`).
