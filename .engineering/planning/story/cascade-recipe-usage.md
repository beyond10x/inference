---
format: aep.planning-md/3
id: story:cascade-recipe-usage
kind: story
status: implemented
title: The voice cascade recipe keeps the usage record of each of its model calls
summary: AgentNode and SpeechNode get a usage port, and voice_cascade exposes the usage of stt, agent and tts
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
- depends_on: story:chat-recipe-usage
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-23T12:35:46Z", actor: "human:operator", revision: 2, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-11-23T12:35:47Z", actor: "human:operator", revision: 3, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-11-23T12:35:47Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Context

`story:chat-recipe-usage` wired the usage port of the model node in `chat`, `transcribe` and
`speak`. `voice_cascade` still drops the usage of all three of its model calls: its `agent` is an
`AgentNode` and its `tts` a `SpeechNode`, and neither has a usage port
(`crates/inference-pipeline/src/nodes/agent.rs` module documentation: "It has no usage port and
sends no usage record"; `crates/inference-pipeline/src/nodes/speech.rs` ignores
`SynthesisEvent::Usage`). Its `stt` node supports one, but the recipe is held equal to the
reference cascade (`crates/inference/tests/fixtures/reference_cascade.rs`), which declares none.
The session total of a cascade run therefore holds no record and reads as complete.
`crates/inference/tests/recipes.rs` names the three nodes in `USAGE_GAPS`.

## Planned scope

- `AgentNode::with_usage` and `SpeechNode::with_usage`, each sending the usage record of every
  model call on its `usage` port, including the usage a superseded or cancelled generation still
  delivers, as `ModelNode` does (`story:llm-usage-on-cancel`).
- `voice_cascade` and the reference cascade declare and expose a `usage` port on `stt`, `agent`
  and `tts`; the run builds each node with it.
- `USAGE_GAPS` becomes empty.

## Acceptance

- One completed spoken turn through `Agent::voice()` over the cascade mocks yields one
  `Event::Usage` per model call (STT, LLM, TTS), and the session total holds those records.
- `USAGE_GAPS` in `crates/inference/tests/recipes.rs` is empty and its test passes.
