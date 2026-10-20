---
format: aep.planning-md/3
id: story:llama-server-tool-proposals
kind: story
status: draft
title: 'llama-server adapter: tool proposals as structured events across a round trip'
summary: tool_calling claimed behind an option; streamed tool calls become ProposeToolCall on the generation id, and the result returns on the same generation.
relations:
- decomposes: epic:local-llm-adapter
- serves: vision:inference
- depends_on: story:llama-server-adapter
revision: 2
---
## Context

`epic:local-llm-adapter` asks that the llama-server adapter emit tool proposals as structured
events, mapped to `inference.tools.ProposeToolCall` and never user-visible prose
(`docs/design/inference.md:374`), and that generation ids survive a tool round trip (line 376).
Executing tools belongs to tools-and-turns and is not in this story.

What exists, read at `llm` tag 0.8.0 and in this repository:

- `llm-chat` sends declared tools as Chat Completions `tools`, reads streamed `tool_calls` deltas
  into `llm` tool calls, refuses a call whose identifier or name never arrived, and sends a tool
  result back as a `tool` message with its `tool_call_id` (`crates/llm-chat/src/incoming.rs:140-260`,
  `crates/llm-chat/src/outgoing.rs:140-161`). A `tool_calls` finish reason is
  `StopReason::ToolCalls` (`incoming.rs:465`).
- `GenerationHandle::take_proposal` yields the calls a turn ended with, once its text is delivered;
  `integrate` answers them and the next turn runs on the same generation
  (`crates/inference-llm/src/lib.rs:923-980`). Behind the `tools` feature, `inference_llm::tools`
  maps the name of a call to the id of its tool and declared tools to `llm` tool specifications
  (`crates/inference-llm/src/tools.rs`).
- `Generator::new` refuses a `tool_calling` claim over a model whose `Capabilities` do not declare
  `tools` (`crates/inference-llm/src/lib.rs:641-665`).
- llama-server emits structured tool calls only when started with its Jinja chat templates
  (`--jinja`), and only for chat templates llama.cpp parses for tool calls. Whether the candidate
  checkpoint of `story:llama-server-adapter` does so on the server build used is checked by the
  live test, not assumed.

## Domain relations

- `inference.tools.ToolCall -> inference.stream.Generation`: many-to-one, a proposal references the
  generation that made it, and the generation stands on its own. Inferable from
  `ess/domains/tools.yaml`, entity `inference.tools.ToolCall`, relation `generation` (references,
  cardinality one, via `generation_id`).
- `inference.tools.ToolCall -> inference.tools.Tool`: many-to-one, references. Inferable from
  `ess/domains/tools.yaml`, entity `inference.tools.ToolCall`, relation `tool` (references,
  cardinality one, via `tool_id`).

## Planned scope

- A `tools` option in the adapter configuration of `story:llama-server-adapter`: when on, the
  `Capabilities` of the binding declare `tools` (and `tool_choice`), the adapter record claims
  `tool_calling` beside `output_streaming`, and a crate feature enables the `tools` feature of
  `inference-llm`. Off, the record is unchanged.
- Offline tests against the loopback fixture server, replaying pinned llama-server stream bytes
  with `tool_calls` deltas split over several chunks:
  - prose then a call in one turn: the prose arrives as segments, the call is taken with
    `take_proposal` as a structured `llm` tool call whose arguments are JSON, and no segment of the
    generation contains the name or the arguments of the call;
  - the call is proposed through `inference.tools.ProposeToolCall` with the id of the generation,
    and the resulting `ToolCall` carries that `generation_id`;
  - a result integrated on the handle starts the next turn on the same generation id; the second
    request the fixture receives carries the assistant `tool_calls` message and a `tool` message
    whose `tool_call_id` equals the id of the proposed call; the generation then completes;
  - a call with no name in any fragment is refused: the generation is cancelled with the `llm`
    protocol error kept and nothing is proposed;
  - the adapter declaration with `tools` on lists exactly `output_streaming` and `tool_calling`,
    and the capability record of `story:llama-server-adapter` gains evidence for `tool_calling`
    naming the tests above, which the testkit check of `story:capability-record` accepts.
- A live test, `#[ignore]` and behind feature `live`, reading `INFERENCE_LLAMA_SERVER_URL` and
  `INFERENCE_LLAMA_SERVER_MODEL` and returning at once when either is unset: it declares one
  read-only tool, asks a question that needs it, asserts a structured proposal for that tool,
  integrates a fixed result and asserts the generation completes with text on the same generation
  id. It prints the server build it ran against.

## Acceptance

`cargo test -p inference-llama-server --all-features` passes offline with the prose-then-call,
`ProposeToolCall` correlation, round-trip and nameless-call tests named above, with no tool-call
text in any segment; with the two variables set against a `llama-server` started with `--jinja`,
the live tool test completes one round trip on one generation id.

## Out of scope

Executing tools, authority and side-effect classes (tools-and-turns); constrained decoding
(section 7.2, line 375).
