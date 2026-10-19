---
format: aep.planning-md/3
id: story:tool-authority-and-executor-nodes
kind: story
status: draft
title: Authorize and execute a read-only call through the mock executor node
summary: A tool authority node (schema validation, default policy, confirmation hook) and a tool executor node generic over the inference-tools executor contract, with the mock executor in inference-testkit.
relations:
- decomposes: epic:tools-and-turns
- depends_on: story:tool-call-lifecycle
- serves: vision:inference
- depends_on: story:text-pipeline-end-to-end
scope:
- confidence: inferred
  path: Cargo.lock
- confidence: cited
  path: crates/inference-pipeline/Cargo.toml
- confidence: inferred
  path: crates/inference-pipeline/src/lib.rs
- confidence: cited
  path: crates/inference-pipeline/src/nodes
- confidence: cited
  path: crates/inference-pipeline/src/nodes/mod.rs
- confidence: cited
  path: crates/inference-pipeline/src/nodes/tool_authority.rs
- confidence: cited
  path: crates/inference-pipeline/src/nodes/tool_executor.rs
- confidence: inferred
  path: crates/inference-runtime/Cargo.toml
- confidence: cited
  path: crates/inference-runtime/tests/tool_nodes.rs
- confidence: inferred
  path: crates/inference-testkit/src/lib.rs
- confidence: cited
  path: crates/inference-testkit/src/mock_tool_executor.rs
revision: 17
---
## Context

§15.1 (`docs/design/inference.md:835-843`): a proposal passes schema validation and authorization
before an executor runs it and a result or failure comes back correlated. Authorizing a tool is a
control decision serialized by a clear authority (699); confirmation is a policy node type (466)
and tools are a node type (467); `inference.graph.NodeRole` has `policy` and `tool`
(`ess/domains/graph.yaml`). Tool arguments and results are Structured modality (294;
`inference.model.Modality`). Tool proposals and outcomes need reliable delivery and correlation
with no silent loss (687). The mock executor is step 5 of the first demonstrator (1175) and lives
in `inference-testkit`.

It runs after `story:tool-call-lifecycle` (the types, the executor contract and the target) and
`story:text-pipeline-end-to-end`, which brings the typed builder of `story:pipeline-declaration`,
the bounded edges of `story:bounded-edges`, the node-instance trait of
`story:node-instance-lifecycle`, the mock source and collector sink and the runtime loop that
drives a run; both `depends_on` edges record it.

## Planned scope

- The tool authority node (role `policy`): an input port for proposed calls and an output port for
  authorized calls, both Structured, reliable delivery. For each proposal: validate `arguments`
  against the Tool schema (848) and `RejectToolCall` with `schema_invalid` on failure; otherwise
  the default policy: `AuthorizeToolCall` when the tool does not require confirmation; when it
  does, call without confirmation, receive `unconfirmed`, hold the call Proposed and expose a
  confirmation hook (a channel or callback a host may answer; no UI, 853, 1054) that authorizes
  on confirmation. Only this node issues authorize and reject commands in a pipeline (699).
- The tool executor node (role `tool`): an input port for authorized calls and an output port for
  results and failures, generic over the executor contract of `inference-tools`. For each call:
  `StartToolCall`, execute under the deadline (852), then `CompleteToolCall` with the result or
  `FailToolCall` with the typed failure; every event carries the call_id and generation_id of the
  proposal (849, 315). The node lives with the node implementations graph-minimum established
  (`inference-pipeline`, nodes, line 241); `inference-tools` does not depend on
  `inference-pipeline` (240).
- The mock executor in `inference-testkit`: constructed with a table of tools, each with a scripted
  result or failure and a deadline behaviour; records every invocation with its call_id (the log
  the retry and interruption stories read).
- The test pipeline on the graph API of graph-minimum: the typed builder and bounded edges, the
  mock source of `story:text-pipeline-end-to-end` scripted as the proposer (Structured payloads),
  authority, executor, that story's collector sink, its runtime loop driving the run. Cases: a
  read-only proposal with valid arguments; a schema-invalid proposal; a tool with
  `requires_confirmation` held until the hook confirms; a tool that fails.

## Where it lands

- `crates/inference-pipeline/src/nodes/tool_authority.rs` and
  `crates/inference-pipeline/src/nodes/tool_executor.rs`, new files, one per node, so that
  `story:tool-feedback-loop` (the bound, in the authority) and `story:tool-safe-retries` (the retry
  policy, in the executor) edit different files. This story creates
  `crates/inference-pipeline/src/nodes/mod.rs` with its two `mod` lines and adds the
  `inference-tools` dependency to `crates/inference-pipeline/Cargo.toml`;
  `story:endpoint-controller` is ordered behind it and adds its own two `mod` lines and the
  `inference-audio` dependency to those two files.
- `crates/inference-testkit/src/mock_tool_executor.rs`, new; `story:tool-safe-retries` and
  `story:tool-call-interruption` extend it, in that order.
- `crates/inference-runtime/tests/tool_nodes.rs`, beside the A-02 test.

## Domain relations

- ToolCall references Tool and Generation, one each: `ess/domains/tools.yaml`,
  `inference.tools.ToolCall`, relations `tool` and `generation`. The executor executes the call for
  the tool the call names; the result carries the generation the call names.
- Node owns Port, many: `ess/domains/graph.yaml`, `inference.graph.Node`, relation `ports`. Both
  nodes declare their ports through `AddPort` with modality Structured.
- Edge references Port as source and sink: `ess/domains/graph.yaml`, `inference.graph.Edge`,
  relations `source` and `sink`; the proposer to authority, authority to executor and executor to
  sink edges are bounded (`capacity > 0`) with overload `fail` (687: never silently lost).
- UNMAPPED: Node to Tool. No `relations:` entry says which tools an executor node serves or
  whether a pipeline may hold several executor nodes; the design lists single-function nodes and
  bridges to many tools as node types (467). Not needed here: the call names its tool (relation
  above), the P3 pipelines hold one executor node that serves every declared tool (topology B,
  769), and a call for a tool the executor was not constructed with is a typed failure. Settle it
  with the first real executor adapter (§15.2).

## Acceptance

In the test pipeline a read-only proposal with valid arguments reaches Completed with a
ToolCallResult carrying the call_id and generation_id of the proposal and one entry in the mock
invocation log, a schema-invalid proposal is Rejected with reason `schema_invalid` and never
reaches the executor, a proposal for a tool that requires confirmation stays Proposed until the
hook confirms it and then completes, and a scripted tool failure leaves the call Failed with a
ToolCallFailed, the nodes honouring `inference.tools.AuthorizeToolCall/outcome/authorized`,
`inference.tools.AuthorizeToolCall/outcome/unconfirmed`,
`inference.tools.RejectToolCall/outcome/rejected`, `inference.tools.StartToolCall/outcome/started`,
`inference.tools.CompleteToolCall/outcome/completed` and
`inference.tools.FailToolCall/outcome/failed`.

## Out of scope

The LLM side of the loop and the feedback edge (`story:tool-feedback-loop`); retries
(`story:tool-safe-retries`); behaviour on interruption (`story:tool-call-interruption`); a real
executor.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). The tree holds no code and no commits (`crates/`,
root `Cargo.toml` and `Cargo.lock` absent; `git log`: no commits yet), so every path below is one
the plan names, not one on disk.

- **Primary surface:** `crates/inference-pipeline/src/nodes` — cited: Where it lands creates the directory with `mod.rs` and one file per node; node implementations live in `inference-pipeline` (`docs/design/inference.md:241`; `story:node-instance-lifecycle` step 2 names the tool-authority and executor nodes as landing in that crate)
- **Files:** `crates/inference-pipeline/src/nodes/tool_authority.rs`, `crates/inference-pipeline/src/nodes/tool_executor.rs` (new, one per node), `crates/inference-pipeline/src/nodes/mod.rs` (new, its two `mod` lines), `crates/inference-pipeline/Cargo.toml` (one `inference-tools` dependency line) — cited: Where it lands; `story:endpoint-controller` names the same `mod.rs` and `Cargo.toml` from its side and carries a `depends_on` edge to this story
- **Files:** `crates/inference-testkit/src/mock_tool_executor.rs` — cited: Where it lands, new; the retry and interruption stories extend it, in that order, behind this one
- **Files:** `crates/inference-runtime/tests/tool_nodes.rs` — cited: Where it lands, new, in the directory `story:text-pipeline-end-to-end` puts the A-02 test in (that body names only `crates/inference-runtime/tests/`, so no file is shared)
- **Symbols (new, unnamed):** the authority node (role `policy`), the executor node (role `tool`), the confirmation hook and the mock executor carry no Rust identifier in the body — cited
- **Symbols (consumed, created by `story:tool-call-lifecycle` in `inference-tools`, in no file yet):** `AuthorizeToolCall` (outcomes `authorized`, `unconfirmed`), `RejectToolCall` (`rejected`, reason `schema_invalid`), `StartToolCall`, `CompleteToolCall`, `FailToolCall`, `ToolCallResult`, `ToolCallFailed`, `Tool.schema`, the executor trait — cited
- **Symbols (consumed, declared today):** `inference.tools.ToolCall` fields `generation_id`, `arguments` and relations `tool`, `generation` (`ess/domains/tools.yaml:94-119`), `inference.tools.Tool.requires_confirmation` (`:63-64`); `inference.graph.NodeRole` variants `policy`, `tool` (`ess/domains/graph.yaml:49-51`), `inference.graph.Node` relation `ports` (`:225`), `inference.graph.AddPort` input `modality` (`:684-693`), `inference.graph.Edge` fields `capacity`, `overload`, invariant `capacity > 0`, relations `source`, `sink` (`:291-305`); `inference.model.Modality` variant `Structured` (`ess/domains/model.yaml:50-52`; design line 294); the typed builder, node-instance trait, `InputEndpoint`/`OutputEndpoint`, mock source, collector sink and runtime loop of the graph-minimum stories — cited
- **Also likely:** `crates/inference-pipeline/src/lib.rs` — inferred: the `mod nodes;` line a new module directory needs; the body does not say it, and `story:pipeline-declaration`, which creates the crate, pre-declares no `nodes` module
- **Also likely:** `crates/inference-testkit/src/lib.rs` — inferred: a `mod mock_tool_executor;` line for a file at the top of `src/`; `story:testkit-conformance-target` step 1 pre-declares nine modules "so that no later story adds a line to `lib.rs`" and this is not one of them; the other reading is `crates/inference-testkit/src/tools/mock_tool_executor.rs`, declared from the pre-declared `tools` module as `story:endpoint-controller` does with `src/stt/synthetic_audio.rs`, which leaves `lib.rs` untouched and moves the file named above
- **Also likely:** `crates/inference-runtime/Cargo.toml` — inferred: a `[dev-dependencies]` line on `inference-tools`, because the test asserts on `ToolCallResult` and `ToolCallFailed` and a crate does not see its dependencies' dependencies; `story:text-pipeline-end-to-end`, the first story to put a test in `crates/inference-runtime/tests/`, names no tools dependency and needs none
- **Also likely:** `Cargo.lock` — inferred: a new dependency edge regenerates it; `.gitignore` (`/target`, `/.engineering/drafts/`) does not exclude it and no story says whether it is committed
- **Not touched:** `ess/domains/tools.yaml` and `ess/domains/graph.yaml` (read; every noun exists, and the body defers the Node to Tool relation to §15.2), `ess/SKIPPED.md` (the acceptance is a Rust integration test; the six scenario ids it names are made to pass by `story:tool-call-lifecycle`), `crates/inference-tools`, `crates/inference-testkit/tests/target/tools.rs`, `crates/inference-testkit/tests/conform.rs` (that story's surfaces), root `Cargo.toml` (`members = ["crates/*"]`, `story:workspace-skeleton` step 1), `Taskfile.yml`, `AGENTS.md`, `README.md` (no step names them; `task check` already runs `cargo test --workspace --all-features`, which discovers a new `tests/*.rs` file) — cited
- **Documents:** none — cited: no step or acceptance names a document
- **Confidence:** high — Where it lands names all six files by path, `story:endpoint-controller` confirms the two shared ones from its side with a `depends_on` edge, and the inferred lines are two module lists, one dev-dependency line and the lock file, each additive — cited
- **Would collide with:** any unit creating or adding a `mod` line to `crates/inference-pipeline/src/nodes/mod.rs` or a dependency line to `crates/inference-pipeline/Cargo.toml` (the later node stories do, each ordered behind this one); any unit adding a module line to `crates/inference-pipeline/src/lib.rs` or `crates/inference-testkit/src/lib.rs`; any unit writing `crates/inference-testkit/src/mock_tool_executor.rs`; any unit adding a dev-dependency to `crates/inference-runtime/Cargo.toml` or regenerating `Cargo.lock`; and, without a shared file, any unit changing the executor trait or the ToolCall command types in `inference-tools`, the node-instance or endpoint traits, or the constructors of the testkit's mock source and collector sink — an API break, not a textual conflict — cited
- **Safety fact:** every existing file this story edits gains lines and loses none (two module lists, one or two manifests), and the one new dependency edge, `inference-pipeline` on `inference-tools`, runs the way the dependency table allows: `inference-tools` depends on `core` only, so no cycle forms and no existing caller changes; the conformance target and `ess/SKIPPED.md` are untouched, so `tests/conform.rs` cannot go red from this story — step 2 (`docs/design/inference.md:240-241`; `story:tool-call-lifecycle`: "Depends on `inference-core` only"), unproven: no manifest exists to point at. Where the symbol search stops: the schema dialect of `Tool.schema: Json` that `arguments` is validated against is named nowhere, and the acceptance's six scenario ids are satisfied by the testkit target of `story:tool-call-lifecycle`, not by these nodes, so a green `conform.rs` says nothing about them — cited
