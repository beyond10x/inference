---
format: aep.planning-md/3
id: epic:tools-and-turns
kind: epic
status: implemented
title: Tools and turns (P3)
summary: The tool contract and lifecycle, a mock executor node, the endpoint controller and the provisional-input barrier; a read-only tool runs in the cascade.
relations:
- derived_from: architecture-design:inference
- depends_on: epic:local-voice-cascade
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T07:56:16Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-10-20T07:56:16Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-10-20T07:56:19Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
## Context

Stage P3, "tools and turns" (`docs/design/inference.md:1161`): commit barriers, a tool node, an
endpoint controller, safe retries; step 5 of the first demonstrator: "add a mock tool executor to
verify feedback, cancellation, and correlation" (line 1175). Tool calling is a graph-level capability
outside model adapters (§15, lines 829-866; ADR-008, line 1193): separate proposal, authorization,
execution and result events (850); read-only, idempotent write and non-idempotent write classes
(851); a provisional transcript never initiates a side-effecting call (866); stopping a model does
not roll back a completed write (856). Loom is treated as absent (owner, 2026-10-18): the executor
here is the design's own contract with a mock behind it.

The exit condition the design gives: "local voice agent executes a read-only tool correctly"
(line 1161).

## Outcome

`inference-tools` holds the tool contract; the cascade of local-voice-cascade proposes a read-only
tool call from committed input, has it authorized, executed by a mock executor node and its result
reinjected with the generation correlation intact, while a side-effecting call on provisional input
is refused before it exists.

## Scope

- `inference-tools` split out of the contracts (§5.3): Tool, ToolCall with the full lifecycle
  Proposed, Authorized, Started, Completed, Failed, Rejected, declared first in
  `ess/domains/tools.yaml` (today one state and the barrier).
- The tool executor node and the feedback edge LLM to executor to LLM with a bounded iteration rule
  (§10.3, line 546).
- The endpoint controller: a VAD node and a turn policy node (§9.1 line 466; §14.2) that commit
  turns (`CompleteTurn`).
- Safe retries: tool retries distinct from generation retries (line 854); idempotency keys for
  idempotent writes (line 1215); no exactly-once claim (line 81).

## Exit

- A-09 (line 1145): proposals are schema-validated, authorized, correlated and auditable; the
  scenarios `inference.tools.ProposeToolCall/outcome/barred` and
  `inference.tools.ProposeToolCall/outcome/proposed` pass, and the lifecycle scenarios added with
  the specification extension pass.
- §23.1 test 5 (line 1109): uncommitted text cannot cause a side-effecting tool by default.
- §23.3 test 6 (line 1129): a transport reconnect does not execute a tool twice.
- The cascade executes one read-only tool through the mock executor and speaks the result.

## Out of scope

MCP or any real executor (adapters later, §15.2); human confirmation UI (the hook exists, line 853,
the UI does not); tool execution while the user talks (duplex-research, §15.3).
