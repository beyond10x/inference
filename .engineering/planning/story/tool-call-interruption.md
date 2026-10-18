---
format: aep.planning-md/3
id: story:tool-call-interruption
kind: story
status: draft
title: Interruption during a tool round trip
summary: Superseding a generation rejects its pending calls, cancels a started one without reinjection and leaves a completed write completed and audited; nothing is rolled back.
relations:
- decomposes: epic:tools-and-turns
- depends_on: story:tool-feedback-loop
- serves: vision:inference
- depends_on: story:tool-safe-retries
scope:
- confidence: cited
  path: crates/inference-pipeline/src/nodes/tool_authority.rs
- confidence: cited
  path: crates/inference-pipeline/src/nodes/tool_executor.rs
- confidence: inferred
  path: crates/inference-runtime
- confidence: inferred
  path: crates/inference-runtime/tests/tool_call_interruption.rs
- confidence: inferred
  path: crates/inference-testkit/src/llm.rs
- confidence: cited
  path: crates/inference-testkit/src/mock_tool_executor.rs
- confidence: cited
  path: crates/inference-tools
revision: 11
---
## Context

On interruption the runtime marks the generation superseded, signals cancellation to affected
node instances, rejects stale results at irreversible sinks and preserves tool audit state while
reconciling already executed side effects separately (`docs/design/inference.md:632-639`);
cancellation of delivery is distinct from cancellation of computation (641); the framework
preserves generation ids and cancellation status across tool round trips (376) and never assumes
stopping a model rolls back a completed write (856). The first demonstrator verifies cancellation
with the mock executor (1175).

It runs after `story:tool-feedback-loop` (the loop pipeline and the LLM node's tool ports) and
`story:tool-safe-retries`; both `depends_on` edges record it. The second edge is an ordering, not
a need: both stories edit the executor node
(`crates/inference-pipeline/src/nodes/tool_executor.rs`), the executor contract in
`inference-tools` and the mock executor (`crates/inference-testkit/src/mock_tool_executor.rs`), so
the retry policy lands before the cancellation signal does and no wave holds both. An ordering
was chosen over splitting those files between the two stories because each story needs both the
node and the mock to demonstrate its own acceptance. The LLM node is the model node of
`story:text-pipeline-end-to-end` as `story:tool-feedback-loop` left it;
`story:cascade-read-only-tool` edits it after this story.

## Planned scope

- Authority: on `inference.stream.GenerationSuperseded` or `GenerationCancelled` for a generation
  with calls Proposed or Authorized, `RejectToolCall` with `superseded` for each; a confirmation
  pending on the hook is withdrawn.
- Executor: a Started call whose generation is superseded receives the cancellation signal of the
  executor contract (636, 852); the tool may finish or fail; the call ends Completed or Failed and
  its event is recorded. A `wrong-state` refusal of `StartToolCall` for a call the authority
  rejected as `superseded` in the meantime is dropped by the executor, not failed (Rejected is
  terminal).
- LLM node: a result or failure for a generation that is not Active is not reinjected and starts
  no model request (637, 376); the superseded generation emits no further segments.
- Completed: a call Completed before the supersede stays Completed with its result in the
  `inference.tools.ToolCalls` view (856, 638); nothing is rolled back and the next generation does
  not receive the stale result.
- Mock executor: this story adds hold-until-release to the mock of
  `story:tool-authority-and-executor-nodes` (its failure injection came with
  `story:tool-safe-retries`): a per-tool behaviour that keeps `execute` pending until the test
  releases it, scripted to finish or to fail on release, and a log entry `cancelled` carrying the
  call_id, written when the cancellation signal of the contract fires on a held call. The Started
  case is read from that entry, not inferred from the call's final state.
- Test: the loop pipeline of `story:tool-feedback-loop`, in its own test file beside the loop
  test, with the mock LLM scripted to propose, superseded by `SupersedeGeneration` with the call in
  each of the four states in turn. Proposed: a tool that requires confirmation, the hook left
  unanswered. Authorized: the mock LLM proposes two calls in one round; the executor node takes
  them in order (852, concurrency limits), the mock holds the first Started, so the second is
  Authorized when the supersede fires and is Rejected by the authority before the first is
  released. Started: the held call itself, scripted to finish on release. Completed: the call
  released and Completed before the supersede.

## Domain relations

- ToolCall references Generation, one, not owned: `ess/domains/tools.yaml`,
  `inference.tools.ToolCall`, relation `generation`; `inference.stream.Generation`
  (`ess/domains/stream.yaml`) owns `segments` only, so superseding a generation discards its
  unplayed segments (local-voice-cascade) and leaves its calls in place for audit.
- Generation owns OutputSegment, many: `ess/domains/stream.yaml`, `inference.stream.Generation`,
  relation `segments`.

## Acceptance

Superseding the generation while its call is Proposed, Authorized, Started and Completed
respectively leaves the call Rejected with `superseded`, Rejected with `superseded`, Completed
with a `cancelled` entry carrying its call_id in the mock executor log and no model request after
its result, and Completed unchanged, with no segment of the superseded generation emitted after
`inference.stream.Generation/transition/supersede/by/inference.stream.SupersedeGeneration/superseded`
and the `inference.tools.ToolCalls` view listing all four calls with their states, honouring
`inference.tools.RejectToolCall/outcome/rejected` and
`inference.tools.ToolCall/state/Completed/refuses/inference.tools.FailToolCall`, with the check
below holding.

## Checks

1. The Started case with the held tool scripted to fail on release: the call ends Failed with a
   ToolCallFailed, the same `cancelled` entry precedes it in the log, and nothing is reinjected.

## Out of scope

Reconciliation of an executed side effect (852, later); barge-in itself and the discard of
unplayed audio (local-voice-cascade, A-10).

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). No code exists (0 commits, `crates/` and `Cargo.toml`
absent), so every `crates/` path is one the plan declares, not one on disk.

- **Primary surface:** `crates/inference-pipeline/src/nodes/tool_executor.rs`, the executor node: cancellation signal to a Started call, drop of the `wrong-state` refusal for a call rejected `superseded` — cited (Context: "both stories edit the executor node (`crates/inference-pipeline/src/nodes/tool_executor.rs`)"; Scope bullet "Executor"; design 636, 852)
- **Files:** `crates/inference-testkit/src/mock_tool_executor.rs`, hold-until-release per tool, finish-or-fail on release, the `cancelled` log entry carrying the call_id — cited (Context; Scope bullet "Mock executor"; `review-result:tools-parallel-round-2` records the behaviour as this story's)
- **Files:** `crates/inference-pipeline/src/nodes/tool_authority.rs`, the authority node: `RejectToolCall` with `superseded` on `GenerationSuperseded` or `GenerationCancelled`, withdrawal of a pending confirmation — cited (Scope bullet "Authority" is this story's; the file is the one `story:tool-authority-and-executor-nodes` "Where it lands" names for the authority node, a `depends_on` neighbour two hops up)
- **Files:** `crates/inference-tools`, the executor contract's cancellation signal — cited (Context: "the executor contract in `inference-tools`"); no file: this body names none and `story:tool-call-lifecycle`'s recorded scope has only `crates/inference-tools` cited and `src/lib.rs` inferred
- **Also likely:** `crates/inference-testkit/src/llm.rs`, the LLM node's reinjection guard (Scope bullet "LLM node") and the mock LLM script for two calls in one round (Test bullet, Authorized case) — inferred: no body names the LLM node's file (`review-result:tools-parallel-round-2`, "What I could not establish"); `story:text-pipeline-end-to-end`'s recorded scope places the model node and the mock LLM in this file, itself an inferred entry there, while `story:node-instance-lifecycle` step 2 puts node implementations in `inference-pipeline`
- **Also likely:** `crates/inference-runtime/tests/tool_call_interruption.rs`, the four-state test and Check 1 — inferred: the body says "its own test file beside the loop test" and `story:tool-feedback-loop` names no test file; the directory is where `tool_nodes.rs`, `endpoint_controller.rs` and `cascade_read_only_tool.rs` sit "beside the A-02 test" and is cited in `story:text-pipeline-end-to-end`'s recorded scope; the name follows `cascade_read_only_tool.rs`
- **Also likely:** `crates/inference-runtime`, the propagation of `GenerationSuperseded` and `GenerationCancelled` to the three nodes — inferred: the body's first sentence has the runtime signalling cancellation to affected node instances (design 636) and §12.1 puts cancellation and generation epochs in `inference-runtime` (660, 662); no Scope bullet names a runtime change, so this is where it lands only if it is not node-local
- **Symbols:** `inference.stream.SupersedeGeneration` with outcome `superseded` and event `GenerationSuperseded` (`ess/domains/stream.yaml:414-433,634`), `CancelGeneration` with `GenerationCancelled` (`:436-455,639`), `inference.tools.ToolCall` relation `generation` (`ess/domains/tools.yaml:94-98`), view `inference.tools.ToolCalls` (`:219-232`); `RejectToolCall`, `StartToolCall`, `FailToolCall`, the reason `superseded`, the states Authorized to Rejected are not in `tools.yaml` today (lifecycle `:99-102` is Proposed only) and are `story:tool-call-lifecycle` steps 1-4 — cited, all consumed, none changed
- **Not touched:** `ess/` (no new noun: `superseded` is lifecycle step 4's reason variant, "none adds a state"; the `cancelled` entry is a mock-log record; `ess/SKIPPED.md` unchanged), `docs/`, `AGENTS.md`, `Taskfile.yml`, root `Cargo.toml` — cited
- **Not touched:** `crates/inference-pipeline/src/nodes/mod.rs`, `crates/inference-testkit/src/lib.rs`, `crates/inference-pipeline/Cargo.toml`, `crates/inference-runtime/Cargo.toml` — inferred: every file above exists before this story by its edges, no module is added, and the test needs no dev-dependency the loop test lacks
- **Documents:** none — cited (every bullet lands in Rust; the two domain files are read for relations only)
- **Confidence:** medium — the executor node, the mock executor and `inference-tools` are named in the body and the authority node by a `depends_on` neighbour, but the LLM node, one of the four behaviour bullets, has a file in no body and is placed only through an upstream inferred entry, and the test file is named only as "beside the loop test"
- **Would collide with:** any unit touching `crates/inference-pipeline/src/nodes/tool_executor.rs` or `tool_authority.rs`; `crates/inference-testkit/src/mock_tool_executor.rs`; the executor contract in `crates/inference-tools`; the model node or mock LLM in `crates/inference-testkit/src/llm.rs`; and, without sharing a file, any unit changing the ToolCall terminal set or the rejection-reason enum (`crates/inference-tools`, `ess/domains/tools.yaml`) or how generation events reach nodes in `crates/inference-runtime`
- **Safety fact:** Completed, Failed and Rejected are terminal and `reject` leaves only Proposed and Authorized (`story:tool-call-lifecycle` step 1), and `superseded` is a variant of the rejection enum that story declares (step 4), so a supersede can move no Completed call and the no-rollback rule (design 856) is a property of the state machine, not a guard this story adds at each sink; the story adds no `ess/` noun, so the synthesized suite and `ess/SKIPPED.md` are unchanged — step 1 for the machine (stated in the lifecycle body; `ess/domains/tools.yaml:99-102` still has Proposed only, so no `file:line` exists for it), unproven. Where the symbol search stops: which ToolCall store the authority and executor share, so that `StartToolCall` on a call just rejected `superseded` answers `wrong-state` instead of starting (named nowhere); how `GenerationSuperseded` reaches the three nodes (design 636, no body places it); the `cancelled` log entry, a mock-internal record with no `ess/` noun — unproven
