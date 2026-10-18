---
format: aep.planning-md/3
id: story:tool-call-lifecycle
kind: story
status: draft
title: ToolCall lifecycle in the specification and the inference-tools crate
summary: Extend ess/domains/tools.yaml with Proposed, Authorized, Started, Completed, Failed, Rejected and their commands, split out inference-tools, and make the testkit target pass every inference.tools scenario.
relations:
- decomposes: epic:tools-and-turns
- serves: vision:inference
- depends_on: story:testkit-conformance-target
scope:
- confidence: inferred
  path: crates/inference-testkit/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/src/tools.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/tools.rs
- confidence: cited
  path: crates/inference-tools
- confidence: inferred
  path: crates/inference-tools/Cargo.toml
- confidence: inferred
  path: crates/inference-tools/src/lib.rs
- confidence: cited
  path: ess/SKIPPED.md
- confidence: cited
  path: ess/domains/tools.yaml
revision: 11
---
## Context

Stage P3 (`docs/design/inference.md:1161`) and §15 (lines 829-868): the tool subsystem provides
separate proposal, authorization, execution and result events (line 850), call and generation
correlation ids (849), read-only, idempotent-write and non-idempotent-write classes (851), human
confirmation where policy requires it (853) and never assumes that stopping a model rolls back a
completed write (856). Today `ess/domains/tools.yaml` declares `inference.tools.ToolCall` with one
state, Proposed, and the provisional-input barrier (`inference.tools.ProposeToolCall/outcome/barred`);
decision D7 of `executable-system-specification:inference` left the rest to this epic.
`inference-tools` is split out of the contracts when its contract is clear (§5.3, line 273); the
dependency table gives it `core`, optionally `text` (line 240).

This story runs after `story:testkit-conformance-target` (`depends_on`): the conformance target,
its in-memory stores, `crates/inference-testkit/tests/conform.rs` and `ess/SKIPPED.md` exist
before this story extends them.

## First step: extend the specification

Before any Rust, extend `ess/domains/tools.yaml` and validate.

1. `inference.tools.ToolCall` lifecycle: states Proposed, Authorized, Started, Completed, Failed,
   Rejected; initial Proposed; terminal Completed, Failed, Rejected; transitions `authorize`
   Proposed to Authorized, `reject` Proposed or Authorized to Rejected, `start` Authorized to
   Started, `complete` Started to Completed, `fail` Started to Failed. Read from lines 835-843 (the
   §15.1 diagram) and 338 (the event family ToolCallProposed, ToolCallAuthorized, ToolCallStarted,
   ToolCallResult, ToolCallFailed).
2. One command per transition, each with a `wrong-state` outcome and a typed state-conflict error
   on the pattern of `inference.stream.CompleteTurn`: `AuthorizeToolCall` (emits
   ToolCallAuthorized), `RejectToolCall` (input `reason`, emits ToolCallRejected), `StartToolCall`
   (emits ToolCallStarted), `CompleteToolCall` (input `result: Json`, emits ToolCallResult),
   `FailToolCall` (input a typed failure, emits ToolCallFailed; read from 852 deadlines and
   cancellation, 1006 typed errors).
3. The confirmation hook, not the UI (853, 1054): `AuthorizeToolCall` refuses with a typed error
   and leaves the call Proposed when the tool requires confirmation and none was given; outcome
   name `unconfirmed`. Whether `requires_confirmation` is copied onto the call at proposal or
   passed in by the authority is this story to decide; the guard must be one `ess` evaluates, as
   `barred` is.
4. A rejection reason enum with variants read from the design: `schema_invalid` (838, 848),
   `denied` (839, 1054), `superseded` (376, 638), `exhausted` (546). Later stories issue these
   reasons; none adds a state.
5. `inference.tools.Tool` gains `schema: Json` (848: schemas and validated argument values) and
   `DeclareTool` sets it; the `inference.tools.ToolCalls` view gains `state` for the audit read
   (A-09, line 1145; 1080).
6. `ess specify validate --path ess` exits 0; `ess verify conform synthesize --path ess --out
   .engineering/drafts/suite.json` reports 0 refusals; record the new scenario count in this body.

No field, state, command or relation beyond the above without a `read from:` line.

## Then the crate and the target

- `crates/inference-tools/`: Tool, ToolCall, SideEffectClass, InputBasis, the lifecycle as a Rust
  state machine, the commands and events above as types, and the executor contract (a trait:
  execute one authorized call under a deadline and a cancellation signal, return a result or a
  typed failure; 852). Depends on `inference-core` only (`inference.stream.GenerationId` lives
  there per Q1 in `executable-system-specification:inference`). Move any tool types P0 left in
  `inference-llm` or `inference-core` here; `inference-llm` takes its optional dependency on this
  crate in `story:tool-feedback-loop`, not here. No executor implementation, no node, no binary.
- `inference-testkit`: the conformance target of `story:testkit-conformance-target` answers
  `ExecuteCommand` and `QueryView` for `inference.tools` too: an in-memory store for Tool and
  ToolCall with the lifecycle above, fed by the `inference-tools` types, and the views
  `inference.tools.Tools` and `inference.tools.ToolCalls`. `ess/SKIPPED.md` loses the three
  `inference.tools` lines that story wrote (`inference.tools.DeclareTool/outcome/declared`,
  `inference.tools.ProposeToolCall/outcome/barred`,
  `inference.tools.ProposeToolCall/outcome/proposed`) and gains none for the scenarios the
  extension adds, so the equality assertion of `tests/conform.rs` holds only when every
  `inference.tools` scenario passes.
- The route is the one that story and the exit of `epic:contracts-and-testkit` establish. On ess
  0.60.0 `ess verify conform run` accepts only its built-in reference targets, so the testkit's
  Rust runner executes the suite synthesized from `ess/`, and
  `ess verify conform report --suite <suite.json> --results <results.json> --report-out <report.json>`
  turns its per-scenario results into the `ess-conformance-report/2` the test asserts on.
- §23.1 test 5 (1109) as a Rust test in `inference-tools`: a provisional-basis proposal of an
  idempotent write is barred; the same proposal on committed basis is proposed.

## Domain relations

- ToolCall references Tool, one, not owned: `ess/domains/tools.yaml`, `inference.tools.ToolCall`,
  relation `tool`. A tool outlives its calls and a call keeps its tool_id after the tool is
  redeclared.
- ToolCall references Generation, one, not owned: `ess/domains/tools.yaml`,
  `inference.tools.ToolCall`, relation `generation`; `inference.stream.Generation` in
  `ess/domains/stream.yaml` declares relations `run`, `producer` and `segments` only, so a
  generation that is superseded or cancelled does not discard its calls (design 638, 856).
- Lifecycle decision taken here, reversible in `tools.yaml`: Failed and Rejected are terminal and
  no transition retries. A tool retry is an attempt inside Started made by the executor; a
  generation retry is a new call from a new generation (854, 1007).

## Acceptance

`cargo test -p inference-testkit --test conform` passes with every `inference.tools` scenario the
extended specification synthesizes in the report's `passed` set, among them
`inference.tools.ProposeToolCall/outcome/barred`,
`inference.tools.ProposeToolCall/outcome/proposed`,
`inference.tools.AuthorizeToolCall/outcome/authorized`,
`inference.tools.AuthorizeToolCall/outcome/unconfirmed`,
`inference.tools.RejectToolCall/outcome/rejected`, `inference.tools.StartToolCall/outcome/started`,
`inference.tools.CompleteToolCall/outcome/completed`, `inference.tools.FailToolCall/outcome/failed`,
`inference.tools.ToolCall/transition/authorize/by/inference.tools.AuthorizeToolCall/authorized`,
`inference.tools.ToolCall/transition/reject/by/inference.tools.RejectToolCall/rejected`,
`inference.tools.ToolCall/transition/start/by/inference.tools.StartToolCall/started`,
`inference.tools.ToolCall/transition/complete/by/inference.tools.CompleteToolCall/completed`,
`inference.tools.ToolCall/transition/fail/by/inference.tools.FailToolCall/failed`,
`inference.tools.ToolCall/state/Proposed/refuses/inference.tools.StartToolCall`,
`inference.tools.ToolCall/state/Started/refuses/inference.tools.StartToolCall` and
`inference.tools.ToolCall/state/Completed/refuses/inference.tools.FailToolCall`, with the checks
below holding.

## Checks

1. `ess specify validate --path ess` exits 0 and `ess verify conform synthesize --path ess --out
   .engineering/drafts/suite.json` reports 0 refusals (step 6). The ids above follow the pattern
   the `Turn` and `Generation` ids have in the suite of 2026-10-18; where the synthesizer spells
   one differently, the list is corrected from the re-synthesized suite, not the suite argued with.
2. `ess/SKIPPED.md` carries no `inference.tools` line.
3. `cargo test -p inference-tools` passes with §23.1 test 5: the provisional-basis proposal of an
   idempotent write barred, the committed-basis proposal proposed.

## Out of scope

Nodes and pipelines (the next stories); any executor beyond the trait; MCP (§15.2, line 860); a
confirmation UI.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `ess/domains/tools.yaml` (the extension, step 1 to 6) and `crates/inference-tools`, the one crate this story creates — cited (body "First step" and "Then the crate"; `docs/design/inference.md:240,273`; `crates/` absent in the tree)
- **Files:** `ess/domains/tools.yaml`: `inference.tools.ToolCall` lifecycle at line 101 (`states: [Proposed]`), `Tool` at 46 gains `schema`, `DeclareTool` at 113, `ToolCalls` view at 219 gains `state`, new commands, events, errors and a reason enum beside `ProposeToolCall` at 144 and `barred` at 157 — cited (steps 1 to 5)
- **Files:** `ess/SKIPPED.md`, the `## inference.tools` section loses its three lines and gains none — cited (body; `story:testkit-conformance-target` step 3 gives the file one section per domain, each story editing only its own)
- **Files:** `crates/inference-testkit/tests/target/tools.rs`, the per-domain target module `story:testkit-conformance-target` step 2 pre-declares as `unsupported` until this story fills it — cited
- **Files:** `crates/inference-testkit/src/tools.rs`, the `tools` store module that story's step 1 pre-declares empty in `lib.rs` — inferred: the depends_on body says "module" and "empty file", not whether `src/tools.rs` or `src/tools/mod.rs`
- **Files:** `crates/inference-testkit/Cargo.toml` gains a dependency on `inference-tools` — inferred: the body says the store is "fed by the `inference-tools` types" and names no manifest; the same file takes one line each from the `stt`, `llm`, `tts` and pipeline-declaration units
- **Also likely:** `crates/inference-tools/Cargo.toml` and `crates/inference-tools/src/lib.rs` — inferred: a library crate needs both and the §23.1 test 5 lives in the crate; the body names only the directory
- **Symbols:** `inference.tools.ToolCall` (states Proposed, Authorized, Started, Completed, Failed, Rejected; transitions authorize, reject, start, complete, fail), `AuthorizeToolCall`, `RejectToolCall`, `StartToolCall`, `CompleteToolCall`, `FailToolCall`, events `ToolCallAuthorized`, `ToolCallRejected`, `ToolCallStarted`, `ToolCallResult`, `ToolCallFailed` (`docs/design/inference.md:338`), outcome `unconfirmed`, the rejection-reason enum `schema_invalid`, `denied`, `superseded`, `exhausted`, `Tool.schema`, `ToolCalls.state`, the executor trait; pattern read from `inference.stream.CompleteTurn` `wrong-state` at `ess/domains/stream.yaml:269-274` and `TurnStateConflict` at 204 — cited
- **Not a surface:** `crates/inference-core`, `crates/inference-llm` — cited: the body's "move any tool types P0 left" clause is conditional, and no P0 body declares one (`story:workspace-skeleton` puts five shared ids in `inference-core`, none a tool id; `story:llm-contract` Out of scope excludes tool proposals and `inference-tools`); `inference-llm`'s dependency on `inference-tools` is `story:tool-feedback-loop`'s by the body's own words
- **Not a surface:** root `Cargo.toml` (`members = ["crates/*"]`, `story:workspace-skeleton` step 1), `crates/inference-testkit/src/lib.rs` and `tests/target/mod.rs` (pre-declared so no later story adds a line, depends_on steps 1 and 2), `crates/inference-testkit/tests/conform.rs` (its equality assertion is what the acceptance rests on; no step edits it), `Taskfile.yml`, `AGENTS.md`, `README.md` (the body names none; `AGENTS.md` Rules already name the tools contract crate), `ess/domains/graph.yaml`, `crates/inference-pipeline/**`, `crates/inference-testkit/src/mock_tool_executor.rs`, `crates/inference-runtime/tests/**` (named only as sibling stories' surfaces) — cited
- **Documents:** `ess/domains/tools.yaml` and `ess/SKIPPED.md` as above; `docs/design/inference.md` and `ess/domains/stream.yaml` (Generation relations at 134-152) are read, not changed; this story's own body gains the scenario count (step 6); `.engineering/drafts/suite.json` is git-ignored (`.gitignore:2`) output, not a surface — cited
- **Confidence:** high — the body names every path, the depends_on story names the per-domain module split and the `SKIPPED.md` section, and each cited design line (240, 273, 338, 374-376, 625, 632-639, 835-856, 1006-1007, 1054, 1080, 1109, 1145, 1161, 1175) reads as the body says; medium only on the two inferred testkit paths
- **Would collide with:** any unit touching `ess/domains/tools.yaml`; `crates/inference-tools` (its executor contract in particular, which the retry and interruption units later edit); the `tools` store module and `tests/target/tools.rs` of the testkit; `crates/inference-testkit/Cargo.toml` (a shared dependency and feature list); and, hunk-adjacent in one file, any unit editing another section of `ess/SKIPPED.md` (the `inference.stream`, `inference.graph`, `inference.policy` sections)
- **Safety fact:** the testkit was split per domain before this story: `lib.rs` and `tests/target/mod.rs` dispatch on the domain prefix and pre-declare the `tools` modules, and `ess/SKIPPED.md` has one section per domain, so this story fills `tools` files and edits one section and touches nothing else the depends_on story writes; the root workspace takes the new crate through `members = ["crates/*"]` with no shared-manifest edit — step 1 (`story:testkit-conformance-target` steps 1 to 3, `story:workspace-skeleton` step 1), unproven, since no file exists yet. Where the symbol search stops: the `unconfirmed` guard has to be a `when:` the ess 0.60.0 synthesizer evaluates over the referenced Tool's `requires_confirmation` or over a field copied onto the call, and the scenario ids and their count come out of `ess verify conform synthesize`, not out of any file in the tree — step 1, unproven
