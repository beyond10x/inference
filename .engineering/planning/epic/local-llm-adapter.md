---
format: aep.planning-md/3
id: epic:local-llm-adapter
kind: epic
status: draft
title: Local LLM adapter, standalone
summary: One self-hosted output-streaming language-model adapter usable with inference-llm alone; tool proposals as structured events.
relations:
- derived_from: architecture-design:inference
- depends_on: epic:contracts-and-testkit
- serves: vision:inference
revision: 2
---
## Context

Step 2 of the first demonstrator: "run one self-hosted output-streaming LLM adapter as a standalone
library" (`docs/design/inference.md:1172`). The LLM contract is §7.2 (lines 364-376): explicit
context semantics; output streaming, incremental input and input-during-output as distinct
capabilities (line 372); structured tool proposals distinguishable from prose (line 374);
generation ids preserved across tool round trips (line 376). A fixed-input text generation must
not be labelled native duplex (line 921).

The backend and hardware are undecided: `decision-blocker:first-adapters`,
`decision-blocker:reference-hardware`, `decision-blocker:weight-licenses`. The review recommends a
local llama.cpp server (`docs/design/review-2026-10-18.md`, F6). The `llm` repository is treated as
absent (owner, 2026-10-18); this adapter speaks to the backend itself.

## Outcome

One self-hosted LLM adapter crate under `adapters/`, usable with `inference-llm` alone, that streams
text deltas, emits tool proposals as structured events, and declares `output_streaming` without
claiming `incremental_input` or `concurrent_input_output`.

## Scope

- One adapter crate implementing the `inference-llm` contract over the chosen backend.
- Its capability record (§17.2); its `DeclareAdapter` row lists exactly the capabilities the §23.2
  tests observe.
- Mapping of the backend's stream to `inference.stream` events: `StartGeneration`, `EmitTextDelta`,
  `CompleteGeneration`, `CancelGeneration`; tool proposals to `inference.tools.ProposeToolCall`.

## Exit

- §23.1 test 2 (line 1106): output streaming and input-during-output verified independently; the
  second is absent and the record says so.
- A-02 (line 1138) with this adapter in place of the mock LLM.
- §23.2 lines 1114-1116 against this adapter; cancellation of delivery is distinct from cancellation
  of computation and both are reported (line 641).

## Out of scope

Hosted LLM adapters (hosted-and-scale-out); tool execution (tools-and-turns); constrained decoding
and embeddings (§7.2 line 375, later capability interfaces).
