---
format: aep.planning-md/3
id: story:example-and-testkit-findings
kind: story
status: implemented
title: Examples, the sherpa recogniser and the duplex mock close the review's remaining findings
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
- serves: vision:inference
scope:
- confidence: inferred
  path: adapters/inference-sherpa-onnx/src/stt.rs
- confidence: inferred
  path: crates/inference-testkit/src/duplex/mock.rs
- confidence: inferred
  path: examples/realtime-voice/src/lib.rs
- confidence: inferred
  path: examples/repo-podcast/src/show.rs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T00:41:05Z", actor: "human:timo", revision: 6}
- {from: "proposed", to: "active", at: "2026-11-06T00:41:05Z", actor: "human:timo", revision: 7}
- {from: "active", to: "implemented", at: "2026-11-06T16:31:05Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":1,"review_outcome":2}}}
---
## Context

From review-result:full-codebase-2026-11-06 (findings N29, N32, N34, N35, N39), at release 0.16.0:

- N29 `examples/realtime-voice/src/lib.rs:441-446`, `:521-563`: after a GPT-Live barge-in, later audio of the stopped response is re-queued and played (design §11.4 step 4). story:cut-response-audio-is-told-from-the-next-words changed this on the wave branch; check what remains.
- N32 `adapters/inference-sherpa-onnx/src/stt.rs:779-782`: tail padding allocates from the caller-supplied rate; `u32::MAX` Hz allocates about 11 GB.
- N34 `examples/repo-podcast/src/show.rs:370-385`: a failed `create_response()` drops the opened session without `close()`.
- N35 `crates/inference-testkit/src/duplex/mock.rs:641-643`: a ledger refusal of a scripted step is discarded and the step consumed.
- N39 `AGENTS.md`: the release tree named the `llm` tag 0.8.3; `main` already says 0.20.0.

## Planned scope

- N29: a test that audio of the cut response after the stop is not played; fix whatever it shows red.
- N32: refuse a rate outside the recogniser's supported set before allocating.
- N34: close the session when `create_response()` fails.
- N35: a ledger refusal of a scripted step fails the scenario with the refusal.
- N39: nothing to change; record the `main` line.

## Acceptance

- Cut-response audio after a GPT-Live stop never plays.
- An absurd rate is refused with a typed error and no allocation.
- A failing `create_response()` leaves the ledger's closing events recorded.
- A mis-scripted `Reply::Call` naming an unoffered tool fails the mock run.
