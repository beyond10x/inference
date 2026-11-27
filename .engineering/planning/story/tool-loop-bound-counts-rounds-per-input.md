---
format: aep.planning-md/3
id: story:tool-loop-bound-counts-rounds-per-input
kind: story
status: implemented
title: The tool loop's bound counts rounds per input, so a chat with tools does not end at its fifth call
tags:
- from-finding
relations:
- informed_by: review-result:sdk-one-call-adversary-2
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T03:24:21Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T03:24:22Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-27T09:18:12Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Context

The tool loop of the recipes `chat` (given tools) and `native_voice` is bounded by
`MAX_TOOL_ITERATIONS`, 4 (`crates/inference/src/recipe.rs`). The specification says the bound is
"how many times one generation may go round the loop along that edge"
(`ess/domains/graph.yaml:431-433`), and a generation is an epoch that only an interruption
supersedes (`ess/domains/stream.yaml:233-240`). The tool authority counts every call a model
proposes in one generation (`crates/inference-pipeline/src/nodes/tool_authority.rs:725-731`,
pinned by `crates/inference-runtime/tests/adversary1_iteration_bound.rs`), and the model node of
`chat` runs one generation for its whole run
(`crates/inference-pipeline/src/nodes/llm.rs:42-45`).

For a `Chat` this means: the bound counts the tool calls of the whole conversation, across its
turns. The fifth call ends the session, and the conversation's history ends with it. A chat whose
model looks one thing up in every turn answers four turns. The second adversary pass on
story:sdk-one-call found it (review-result:sdk-one-call-adversary-2, finding 2), and its case
`a_chat_whose_model_calls_its_tool_once_a_turn_answers_six_turns` is red on the branch of that story.

A bound of this kind exists to stop a model that calls tools without end inside one answer. It
is not meant to end a conversation that is going well.

## Planned scope

- `ess/domains/graph.yaml`: say what `max_iterations` counts for a model that answers several
  inputs in one generation: the replies that end with calls, per input.
- `crates/inference-pipeline/src/nodes/llm.rs`: the proposals of one reply carry a mark of their
  reply and input (today `causation_id` is `None`).
- `crates/inference-pipeline/src/nodes/tool_authority.rs`: count rounds per generation and input;
  a proposal without the mark counts singly, so `native_voice` and the runtime's tests keep their
  meaning.
- `crates/inference/src/recipe.rs`, `src/chat.rs`: the documentation of the bound.
- The case the adversary wrote, `a_chat_whose_model_calls_its_tool_once_a_turn_answers_six_turns`
  in `crates/inference/tests/adversary2_w2u1_one_call.rs`, is the acceptance test.
- One test states the bound of `native_voice` under the new counting.

## Acceptance

- A chat whose model calls its tool once in every turn answers six turns over one session; a
  turn whose model returns calls a fifth time ends with the error that names the bound, as today
  (state transition: the specification's sentence and the authority's count change).
- The tests of `native_voice` and of `crates/inference-runtime` that pin the bound pass
  unchanged (state transition: the same).
