---
format: aep.planning-md/3
id: story:chat-recipe-usage
kind: story
status: draft
title: The built-in recipes keep the usage record of every model call they run
summary: chat, transcribe and speak expose their model node's usage port, so a completed reply reaches the SDK event stream and the session total
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference/src/recipe/run.rs
- confidence: cited
  path: crates/inference/src/recipe/topology.rs
- confidence: cited
  path: crates/inference/src/stack.rs
- confidence: cited
  path: crates/inference/tests/agent.rs
- confidence: cited
  path: crates/inference/tests/recipes.rs
revision: 2
---
## Context

Review of 0.31.0, finding 6: the built-in `chat` recipe drops the usage record of a completed
reply. In a local run over the mock the model wrapper produced one record
(`inference_llm::usage_of_turn`), the session accountant received none, no node was marked
uncounted, and the SDK emitted no `Event::Usage`. The recipe's `agent` node declares no `usage`
port (`crates/inference/src/recipe/topology.rs`, `chat`) and the run builds its `ModelNode`
without `with_usage` (`crates/inference/src/recipe/run.rs`, `agent`), so the node has nowhere to
send the record (`crates/inference-pipeline/src/nodes/llm.rs`, `send_usage`). The session total
stays empty, and an empty `UsageTotal` is `complete()` (`crates/inference-core/src/usage.rs`).

The same omission holds in `transcribe` (its `stt` node) and `speak` (its `tts` node): their
nodes support a usage port and the recipes declare none. `native_voice` carries usage on its
`events` egress already. `voice_cascade` builds its `agent` as an `AgentNode` and its `tts` as a
`SpeechNode`, neither of which has a usage port; that is a node feature, left to a follow-up.

## Planned scope

- `topology.rs`: `chat`, `transcribe` and `speak` declare a structured output port `usage` on their
  model node and expose it as the egress `usage`, at the recipe's capacity under `pause`.
- `run.rs` and `stack.rs`: a role-bound LLM, STT or plain TTS node whose recipe declares a port
  `usage` is built with that port (`ModelNode::with_usage`, `SttNode::with_usage`,
  `TtsNode::with_usage`); an application recipe that declares one gets it the same way.
- Tests in `crates/inference`: one completed chat turn over the mock, and a check over every
  built-in recipe that each model node's usage reaches an egress, with the cascade's nodes named
  as the one known gap.

## Acceptance

- One completed chat turn on `Agent::text()` over a mock generator that reports usage yields
  exactly one `Event::Usage` on the SDK event stream, carrying `requests` 1 and the mock's token
  counts, and the session's usage total (`LiveSession::usage().session`) holds that one record with
  the same counts.
- `chat`, `transcribe` and `speak` each expose an egress `usage` from their model node's `usage`
  port; `native_voice` keeps its usage on `events`; every other model node of a built-in recipe
  is named in the test's list of known gaps, and the test fails when that list and the recipes
  disagree.
- `cargo test -p inference --features testing` passes, apart from the compile-check fixture
  excluded by the brief.

## Not in scope

- Usage ports on `AgentNode` and `SpeechNode`, and wiring them in `voice_cascade`.
- Making `UsageTotal::complete()` fail for a model node that declares no usage port: the runtime
  cannot tell from a declaration which nodes produce usage (a duplex node carries it on `events`).
