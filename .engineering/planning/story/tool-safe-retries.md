---
format: aep.planning-md/3
id: story:tool-safe-retries
kind: story
status: draft
title: Safe tool retries and at-most-once start
summary: Executor retries by side-effect class with an unchanged call_id as the idempotency key, no automatic retry of a non-idempotent write, and one execution when an authorized call is delivered twice.
relations:
- decomposes: epic:tools-and-turns
- depends_on: story:tool-authority-and-executor-nodes
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-pipeline/src/nodes/tool_executor.rs
- confidence: inferred
  path: crates/inference-runtime/tests/tool_retries.rs
- confidence: cited
  path: crates/inference-testkit/src/mock_tool_executor.rs
- confidence: cited
  path: crates/inference-tools
- confidence: inferred
  path: ess/domains/tools.yaml
revision: 9
---
## Context

Tool retries are distinct from generation retries (`docs/design/inference.md:854`); retrying an
irreversible tool operation may not be safe (1007); the mitigation for duplicated real-world
effects is idempotency keys, confirmation and status reconciliation with no exactly-once claim
(1215, 81, 1000); transports duplicate, lose and reconnect (999) and tools must not execute twice
merely because of a transport reconnect (§23.3 test 6, 1129). Event ids and call ids exist for
deduplication and correlation (308, 849).

## Planned scope

- A retry policy by side-effect class in the executor contract of `inference-tools` and in the
  executor node: a transient failure of a `read_only` or `idempotent_write` call is attempted
  again by the executor, up to a limit the node is configured with, every attempt presenting the
  same call_id to the tool; the call_id is the key the tool deduplicates on (849, 1215). A
  `non_idempotent_write` is never attempted again automatically (1007): one attempt, then Failed,
  left to reconciliation (852) outside this epic. Neither call_id nor generation_id changes across
  attempts; a generation retry is a new call (854). Attempts are not lifecycle transitions: the
  call is Started from the first attempt to its Completed or Failed; the attempt count travels on
  the failure event and on the tool call latency and failure metric (1025).
- At-most-once start: delivering the authorized call to the executor twice (a redelivery, 999)
  yields one `StartToolCall` accepted, one `wrong-state` refusal and one invocation in the mock log.
- The mock executor gains failure injection: fail N times then succeed; fail permanently; record
  every attempt.

## Domain relations

- ToolCall references Tool, one: `ess/domains/tools.yaml`, `inference.tools.ToolCall`, relation
  `tool`; the side-effect class the retry policy reads is the class the call carries.
- The lifecycle has no retry transition (decision recorded in `story:tool-call-lifecycle`):
  Failed is terminal, so a second `StartToolCall` on a Started, Completed or Failed call is
  refused by state.

## Acceptance

With the mock tool scripted to fail once then succeed, a read-only call completes on the second
attempt with unchanged call_id and generation_id and two entries in the mock log, a
non-idempotent-write call scripted the same way is Failed after one entry, and delivering one
authorized call to the executor twice yields one entry and one refusal, honouring
`inference.tools.ToolCall/state/Started/refuses/inference.tools.StartToolCall`,
`inference.tools.ToolCall/state/Completed/refuses/inference.tools.StartToolCall` and
`inference.tools.FailToolCall/outcome/failed`.

## Out of scope

Status reconciliation of a completed or unknown-outcome write (852); a tool-facing idempotency
key distinct from the call_id (add a field when a real executor needs one); any transport.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). No code exists in the tree (`git ls-files` is empty);
every `crates/` path below is one this story's `depends_on` chain declares, not one on disk.

- **Primary surface:** `crates/inference-pipeline/src/nodes/tool_executor.rs` — cited: the retry policy, the attempt limit the node is configured with and the at-most-once start all sit "in the executor node" (body lines 25-35); `story:tool-authority-and-executor-nodes` creates that file and names this story as the one that edits it ("the retry policy, in the executor", `.engineering/planning/story/tool-authority-and-executor-nodes.md:60`); `story:tool-call-interruption` names it as the file both stories edit (`.engineering/planning/story/tool-call-interruption.md:28`)
- **Files:** `crates/inference-testkit/src/mock_tool_executor.rs` — cited: "the mock executor gains failure injection" (body line 36); the authority story creates it and orders this story's extension first (`tool-authority-and-executor-nodes.md:67`); the interruption story records that "its failure injection came with `story:tool-safe-retries`" (`tool-call-interruption.md:52`)
- **Secondary surface:** `crates/inference-tools` — cited: "a retry policy by side-effect class in the executor contract of `inference-tools`" (body line 25); the contract is the trait `story:tool-call-lifecycle` puts in that crate, named by directory only (`.engineering/planning/story/tool-call-lifecycle.md:64-66`; `docs/design/inference.md:240`), so the directory is the granularity
- **Also likely:** `crates/inference-runtime/tests/tool_retries.rs` — inferred: the body names no test file; its acceptance is a scripted-mock pipeline test of the kind every sibling puts in its own file in `crates/inference-runtime/tests/` beside the A-02 test (`tool_nodes.rs`, `tool-authority-and-executor-nodes.md:69`; `endpoint_controller.rs`; `cascade_read_only_tool.rs`; the interruption story's "own test file"), and a new file keeps it out of `tool_nodes.rs`, which a unit of the same wave may extend
- **Also likely:** `ess/domains/tools.yaml` — inferred: "the attempt count travels on the failure event" (body lines 32-33), and `FailToolCall`/`ToolCallFailed` are declared in that file by `story:tool-call-lifecycle` step 2 under its rule "no field ... without a `read from:` line" (`tool-call-lifecycle.md:60`), with `AGENTS.md:39` putting a new noun in `ess/` before code; the body itself names no specification edit
- **Symbols:** `inference.tools.SideEffectClass` with `read_only`, `idempotent_write`, `non_idempotent_write` (`ess/domains/tools.yaml:27-29`), `inference.tools.ToolCall` identity `call_id` and fields `generation_id`, `side_effect` (`:71-83`); `StartToolCall`, `FailToolCall`, `ToolCallFailed` and the `wrong-state` refusal (`story:tool-call-lifecycle` step 2, not yet in the tree); the executor trait of `inference-tools` (unnamed file) — cited; all read, only the trait and the failure event possibly changed
- **Not touched, by the bodies' own text:** `crates/inference-pipeline/src/nodes/tool_authority.rs`, `crates/inference-pipeline/src/nodes/mod.rs`, `crates/inference-pipeline/Cargo.toml` and the testkit module line for the mock (all the authority story's, `tool-authority-and-executor-nodes.md:59-67`); the LLM node, `ess/domains/graph.yaml`, the validator (`story:tool-feedback-loop`); root `Cargo.toml`, `Taskfile.yml`, `AGENTS.md`, `README.md`; any transport (Out of scope) — cited
- **Documents:** `ess/domains/tools.yaml` only, and only if the attempt count becomes a declared field as above — inferred; `docs/design/inference.md` is read, not changed
- **Confidence:** high — the two files this story edits are named for it by its `depends_on` story and repeated by the story ordered behind it, and the crate holding the contract is named in the body; what is read rather than given is a test file name and whether one field reaches the specification
- **Would collide with:** any unit editing the executor node (`crates/inference-pipeline/src/nodes/tool_executor.rs`: cancellation signal, deadline, concurrency, start handling), the mock executor (`crates/inference-testkit/src/mock_tool_executor.rs`: any new scripted behaviour or log entry), the executor trait in `crates/inference-tools` (its signature or failure type), the `FailToolCall`/`ToolCallFailed` declarations in `ess/domains/tools.yaml`, or `crates/inference-runtime/tests/tool_retries.rs`; not with units on the authority node, `nodes/mod.rs`, the pipeline validator, the LLM node or `ess/domains/graph.yaml`
- **Safety fact:** the lifecycle has no retry transition: `start` runs only Authorized to Started and Failed is terminal (`story:tool-call-lifecycle` step 1 and `tool-call-lifecycle.md:98`: "no transition retries. A tool retry is an attempt inside Started made by the executor"), so attempts change no state and a redelivered `StartToolCall` on a Started, Completed or Failed call is refused by the state machine the node already calls, which is why neither the retry policy nor at-most-once start adds a command, a state or an edit to any other node — level 2, unproven: `ess/domains/tools.yaml:99-102` still declares Proposed only. Where the symbol search stops: (a) the unchanged `call_id` deduplicates only if the tool keys on it; the mock does by construction and a real executor adapter (§15.2) is unchecked, with a distinct idempotency key deferred by Out of scope; (b) an attempt count on `ToolCallFailed` is a payload the executor's output port delivers to `llm.tool_result` (`story:tool-feedback-loop`) and that the synthesized scenario `inference.tools.FailToolCall/outcome/failed` would check once declared, so it is a wire shape with two readers, not a private field — level 1, unproven
