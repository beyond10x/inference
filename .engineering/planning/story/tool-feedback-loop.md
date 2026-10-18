---
format: aep.planning-md/3
id: story:tool-feedback-loop
kind: story
status: draft
title: LLM to executor feedback loop with a bounded iteration rule
summary: The LLM node gains tool_call and tool_result ports, the cycle through authority and executor is bounded, the result reinjects on the same generation, and the proposal past the bound is rejected as exhausted.
relations:
- decomposes: epic:tools-and-turns
- depends_on: story:tool-authority-and-executor-nodes
- serves: vision:inference
- depends_on: story:pipeline-validation
- depends_on: story:text-pipeline-end-to-end
scope:
- confidence: cited
  path: .engineering/planning/executable-system-specification/inference.md
- confidence: cited
  path: crates/inference-llm
- confidence: cited
  path: crates/inference-llm/Cargo.toml
- confidence: cited
  path: crates/inference-pipeline
- confidence: cited
  path: crates/inference-pipeline/src/nodes/tool_authority.rs
- confidence: cited
  path: crates/inference-runtime/tests
- confidence: inferred
  path: crates/inference-runtime/tests/tool_feedback_loop.rs
- confidence: inferred
  path: crates/inference-testkit/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/src/graph.rs
- confidence: inferred
  path: crates/inference-testkit/src/llm.rs
- confidence: inferred
  path: crates/inference-testkit/tests/target/graph.rs
- confidence: inferred
  path: ess/SKIPPED.md
- confidence: cited
  path: ess/domains/graph.yaml
revision: 17
---
## Context

The feedback pattern LLM to tool (`docs/design/inference.md:542`), topology A with its optional
tool call and result feedback (749-757) and topology B (765-769). A feedback edge must have a
defined scheduler, queue bounds and a termination or iteration rule; unbounded cycles are invalid
by default (546; 517 feedback loop declarations). Check 5 of §10.2 (529) is implemented by
`story:pipeline-validation` as refusing every cycle
(`inference.graph.ValidatePipeline/outcome/rejected`), because `ess/domains/graph.yaml` has no noun
for a bounded feedback declaration; that story hands the noun to this one, and this story declares
it and amends check 5. Structured tool proposals are distinguishable from prose (374) and the
framework preserves generation ids and cancellation status across tool round trips (376).
`inference-llm` may depend on `inference-tools` optionally (237). For an ordinary text model a tool
result is integrated at a safe boundary, the end of the model request that emitted the proposal,
not concurrently (402, 721).

It runs after `story:tool-authority-and-executor-nodes` (the two nodes and the mock executor),
`story:pipeline-validation` (the validator whose check 5 it amends) and
`story:text-pipeline-end-to-end` (the LLM node, the mock source and sink and the runtime loop);
the three `depends_on` edges record it. The LLM node, here and in every later story of this epic,
is the model node of `story:text-pipeline-end-to-end` wrapping the mock LLM adapter.

## Planned scope

- `inference-llm` takes its optional dependency on `inference-tools`; the LLM node gains a
  `tool_call` output port and a `tool_result` input port (the example edges at 581-584),
  Structured modality. The node turns a structured proposal of its adapter into `ProposeToolCall`
  with `input_basis` committed when every span it generated from is Committed, else provisional;
  a barred proposal is dropped and the generation continues without it.
- The testkit mock LLM scripted for tools: proposes one call and, on its result, continues the same
  generation with text that contains the result; where the P0 mock cannot be scripted this way,
  extend it.
- The cycle: `llm.tool_call` to the authority, authority to executor, executor to
  `llm.tool_result`, each edge bounded with overload `fail` (687). The iteration rule is the
  number of tool rounds one generation may make. Spec first: this story declares the feedback
  bound in `ess/domains/graph.yaml`, where the pipeline declares its feedback loop (517), with
  `read from:` 517 and 546, runs `ess specify validate --path ess` and `ess verify conform
  synthesize --path ess --out .engineering/drafts/suite.json` (0 refusals) and records the counts
  in `executable-system-specification:inference`. Then it amends check 5 of `ValidatePipeline` in
  `inference-pipeline`: a cycle whose edges carry a declared bound validates; a cycle without one
  is still refused with the check-5 finding of `story:pipeline-validation`, whose test for the
  unbounded cycle stays red-capable. The authority enforces the bound: the proposal past it is
  `RejectToolCall` with `exhausted`, and the LLM node, receiving that rejection, completes its
  generation (the termination rule). No new state on ToolCall.
- Reinjection: the result arrives on `tool_result` after the model request that proposed has
  ended; the node issues the next model request with the result appended;
  `inference.stream.Generation` stays Active from the proposal to `CompleteGeneration` (376) and
  every segment after the result carries the same generation_id.
- Later edits to the LLM node are ordered behind this story by their edges: the reinjection guard
  (`story:tool-call-interruption`) and the start trigger (`story:cascade-read-only-tool`), in
  that order.

## Domain relations

- ToolCall references Generation, one: `ess/domains/tools.yaml`, `inference.tools.ToolCall`,
  relation `generation`; the result rejoins that generation and no other.
- Generation references its producer Node, one: `ess/domains/stream.yaml`,
  `inference.stream.Generation`, relation `producer`; the result returns to the node that produced
  the generation.
- Generation owns OutputSegment, many: `ess/domains/stream.yaml`, `inference.stream.Generation`,
  relation `segments`; the text after the result belongs to the same generation.
- Edge references Port as source and sink: `ess/domains/graph.yaml`, `inference.graph.Edge`,
  relations `source` and `sink`.
- Binding references Node and Adapter, one each: `ess/domains/graph.yaml`,
  `inference.graph.Binding`, relations `node` and `adapter`; the LLM node requires `tool_calling`
  (`inference.model.Capability`, design 424) and planning refuses an adapter without it
  (`inference.graph.PlanPipeline/outcome/mismatch`, A-13, graph-minimum).

## Acceptance

In the text-only pipeline on mocks with the feedback edge, the scripted LLM proposal is executed
and its result reinjected on the same generation_id, the text emitted after the result carries
that generation_id and the generation reaches Completed, and with iteration bound N the proposal
number N+1 within one generation is Rejected with reason `exhausted` while the generation still
completes, the pipeline validating with the bound declared and refused without it
(`inference.graph.ValidatePipeline/outcome/rejected`).

## Out of scope

Real adapters (`story:cascade-read-only-tool`); interruption (`story:tool-call-interruption`);
tool execution while the user talks (§15.3, duplex-research).

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). No code exists in the tree (no commits, `crates/`
absent), so every `crates/` path below is one the plan declares, not one on disk.

- **Primary surface:** `crates/inference-pipeline` — cited: body Scope bullet 3 amends check 5 of `ValidatePipeline` "in `inference-pipeline`" (the crate `story:pipeline-declaration` creates, `docs/design/inference.md:241`): a cycle whose edges carry the declared bound validates, one without it is still refused with the check-5 finding of `story:pipeline-validation`; the typed builder of that crate is where a declaration carries the bound
- **Files:** `crates/inference-pipeline/src/nodes/tool_authority.rs` — cited: the authority enforces the bound (body Scope bullet 3, `RejectToolCall` with `exhausted` past it); `story:tool-authority-and-executor-nodes` "Where it lands" creates that file "so that `story:tool-feedback-loop` (the bound, in the authority) and `story:tool-safe-retries` (the retry policy, in the executor) edit different files"
- **Secondary surface:** `crates/inference-llm` — cited: body Scope bullet 1, "`inference-llm` takes its optional dependency on `inference-tools`" (`docs/design/inference.md:237`; `story:tool-call-lifecycle` "Then the crate": that dependency is this story's, "not here"); the adapter's structured proposal the node converts (body: "a structured proposal of its adapter"; design 374, left out of scope by `story:llm-contract`) is the contract change behind the dependency
- **Files:** `crates/inference-llm/Cargo.toml` — cited: the optional dependency line, the one place Cargo declares one; no body names the feature (design 237 calls the optional dependency `tools`)
- **Files:** `ess/domains/graph.yaml` — cited: body Scope bullet 3 declares the feedback bound "where the pipeline declares its feedback loop (517)" with `read from:` 517 and 546; candidate blocks are `types:` (17-79), `inference.graph.Pipeline` (88-131, `read from: 507-517`), `inference.graph.Edge` (213-240), `DeclarePipeline` (388-417), `ConnectPorts` (491-529) and the `ValidatePipeline` summary at 576 ("every cycle is bounded"); which one is the implementor's choice
- **Files:** `.engineering/planning/executable-system-specification/inference.md` — cited: body Scope bullet 3 "records the counts in `executable-system-specification:inference`": the summary (`:7`), `## What it declares` (`:31-34`) and the `## Evidence` table (`:36-43`); written through `aep plan artifact body --section` only (`AGENTS.md` Layout)
- **Files:** `crates/inference-runtime/tests` — cited: the Acceptance runs "the text-only pipeline on mocks with the feedback edge", the integration pipeline of `story:text-pipeline-end-to-end` ("one integration test in `crates/inference-runtime/tests/`"), and `story:tool-call-interruption` Scope puts its own test "in its own test file beside the loop test"
- **Files:** `crates/inference-runtime/tests/tool_feedback_loop.rs` — inferred: the loop test as a new file; no body names it, the name follows `tool_nodes.rs`, `endpoint_controller.rs` and `cascade_read_only_tool.rs` of the sibling stories
- **Files:** `crates/inference-testkit/src/llm.rs` — inferred: the mock LLM scripted for tools (body Scope bullet 2) and the LLM node that gains `tool_call` and `tool_result` (bullet 1), "the model node of `story:text-pipeline-end-to-end` wrapping the mock LLM adapter" (body Context); that story's recorded scope puts mock and node at this path and calls the node's file its own choice; `review-result:tools-parallel-round-2` records that no body names the node's file
- **Also likely:** `crates/inference-testkit/Cargo.toml` — inferred: the testkit's `llm` feature (`llm = ["dep:inference-llm"]`, `story:llm-contract` step 2) has to enable the tools side of `inference-llm` for the mock to emit a structured proposal; no body says so
- **Also likely:** `crates/inference-testkit/tests/target/graph.rs` and `crates/inference-testkit/src/graph.rs` — inferred: a noun declared in `graph.yaml` reaches the testkit's graph-domain target and store (per-domain split, `story:testkit-conformance-target` steps 1-2): as a new input of `ConnectPorts` or `DeclarePipeline` it changes the payload of every existing scenario of that command, as a new command or outcome it adds scenario ids the target must answer; the body names neither file
- **Also likely:** `ess/SKIPPED.md` — inferred: only if the noun synthesizes a scenario id the testkit does not answer, because `tests/conform.rs` asserts `unsupported == ess/SKIPPED.md` (`story:testkit-conformance-target` step 2); the body's "records the counts" expects the count to move
- **Symbols:** ports `tool_call`, `tool_result` (`docs/design/inference.md:581-584`); `inference.tools.ProposeToolCall` (`ess/domains/tools.yaml:144`) with `input_basis` (`:84`, `:152`), `inference.tools.ToolCall` relation `generation` (`:94-98`); `RejectToolCall` reason `exhausted` (absent from `ess/` today, `rg exhausted ess/` 0 hits; promised by `story:tool-call-lifecycle` step 4); `inference.stream.Generation` states Active, Completed (`ess/domains/stream.yaml:154-160`), relations `producer` (`:140`), `segments` (`:148`), `CompleteGeneration` (`:391`); `inference.graph.ValidatePipeline` (`ess/domains/graph.yaml:558`, `rejected` external `:564`), `Edge.capacity`/`overload` (`:226-228`), `OverloadPolicy` `fail` (`:77-79`); `inference.model.Capability` `tool_calling` (`ess/domains/model.yaml:36`), `Modality` `Structured` (`:46`); `inference.graph.PlanPipeline/outcome/mismatch` (`graph.yaml:590-598`), consumed — cited
- **Not touched:** `ess/domains/tools.yaml` and `crates/inference-tools` ("No new state on ToolCall"; the reason enum and `RejectToolCall` are `story:tool-call-lifecycle`'s), `crates/inference-pipeline/src/nodes/tool_executor.rs` and `crates/inference-testkit/src/mock_tool_executor.rs` (the executor side is `story:tool-safe-retries`' and `story:tool-call-interruption`'s by the file split of `story:tool-authority-and-executor-nodes`), `crates/inference-pipeline/src/nodes/mod.rs` and `crates/inference-pipeline/Cargo.toml` (no new node; the `inference-tools` dependency is the authority story's), `crates/inference-testkit/src/lib.rs` (modules pre-declared, `story:testkit-conformance-target` step 1), root `Cargo.toml` (`members = ["crates/*"]`), `Taskfile.yml`, `AGENTS.md`, `README.md`, `ess/system.yaml`, `ess/ess-inputs.yaml` (no new domain document), `docs/design/inference.md` (read only) — cited
- **Documents:** `ess/domains/graph.yaml` and `.engineering/planning/executable-system-specification/inference.md`, both cited above; `ess/SKIPPED.md` conditional as above; `.engineering/drafts/suite.json` is git-ignored output (`.gitignore:2`), not a surface — cited
- **Confidence:** medium — the crates, the specification document and the authority file are read from the body and from `story:tool-authority-and-executor-nodes`, but the LLM node's file is named in no body (`review-result:tools-parallel-round-2`, "What I could not establish"), so its home follows `story:text-pipeline-end-to-end`'s inferred entry, and the shape of the feedback-bound noun decides whether the testkit graph files and `ess/SKIPPED.md` are touched
- **Would collide with:** any unit declaring into `ess/domains/graph.yaml` (`types:`, `Pipeline`, `Edge`, `ConnectPorts`, `DeclarePipeline` or `ValidatePipeline` blocks); the validation half of `crates/inference-pipeline` and `crates/inference-pipeline/src/nodes/tool_authority.rs`; `crates/inference-llm` (manifest and public contract); the testkit's `src/llm.rs` (the two later LLM-node edits, `story:tool-call-interruption` then `story:cascade-read-only-tool`, are ordered behind this story by edges); the testkit's graph-domain files and `ess/SKIPPED.md` if the noun synthesizes a scenario; the summary, counts and `## Evidence` table of `.engineering/planning/executable-system-specification/inference.md` (two contracts-epic stories write the same lines by their recorded scopes); `crates/inference-testkit/Cargo.toml` (the one `[features]`/`[dependencies]` table every testkit unit shares)
- **Safety fact:** the bound is additive and optional: a declaration without it is unchanged, so check 5 keeps refusing every unbounded cycle (design 546, "invalid by default") and `story:pipeline-validation`'s unbounded-cycle test keeps its verdict; and `ValidatePipeline`'s `rejected` outcome is `external` with `validated` unguarded (`ess/domains/graph.yaml:564-577`), so the noun derives no new `ValidatePipeline` scenario — step 2 (`file:line`), unproven. Where the symbol search stops: (1) the wire payload the synthesized suite sends to the testkit's graph target: a new `ConnectPorts`/`DeclarePipeline` input changes every existing scenario of that command, a new command adds ids, and the `unsupported == ess/SKIPPED.md` equality in `tests/conform.rs` turns either into a red test unless the target or the file is edited; (2) the `exhausted` reason, which no `ess/` document declares today and which this story's authority and Acceptance both issue — step 2, unproven
