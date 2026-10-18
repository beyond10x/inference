---
format: aep.planning-md/3
id: story:cascade-read-only-tool
kind: story
status: draft
title: The voice cascade executes a read-only tool and speaks the result
summary: The cascade declaration executes one read-only tool through the barrier and speaks the result; the acceptance runs it mock-bound, the real-model run is a pinned demonstration.
relations:
- decomposes: epic:tools-and-turns
- depends_on: story:tool-feedback-loop
- depends_on: story:endpoint-controller
- serves: vision:inference
- depends_on: story:tool-call-interruption
- depends_on: story:tts-contract
scope:
- confidence: inferred
  path: crates/inference-runtime/Cargo.toml
- confidence: cited
  path: crates/inference-runtime/tests/cascade_read_only_tool.rs
- confidence: inferred
  path: crates/inference-testkit/src/llm.rs
revision: 9
---
## Context

The exit condition of P3: a local voice agent executes a read-only tool correctly
(`docs/design/inference.md:1161`); topology B (759-778) with VAD and endpointing, STT, commit gate,
the LLM with tool call and result through the tool executor, committed text, TTS, playback;
demonstrator step 5 (1175). The run stays offline (A-04, local-voice-cascade; `DeploymentProfile`
`offline_strict`, 1046). Side-effecting tools require committed user input by default (625) and
the LLM in this cascade receives committed spans only, so every call it proposes has `input_basis`
committed; the barrier (866) guards the case where a speculating node proposes.

It runs after `story:tool-feedback-loop`, `story:endpoint-controller` and
`story:tool-call-interruption`; the `depends_on` edges record it. The third is an ordering, not a
need: this story and `story:tool-call-interruption` both edit the LLM node (the model node of
`story:text-pipeline-end-to-end`, given its tool ports by `story:tool-feedback-loop`), one its
start trigger and the other its reinjection guard, so the two never share a wave. An ordering was
chosen over moving the start trigger into an earlier story because the earlier pipelines are
text-only and have no turn to start on. The commit-gate node, the acknowledging audio sink, the
local adapters and the A-04 test are `epic:local-voice-cascade`'s; that epic has no stories yet,
so the epic-level edge is the record.

This story also depends on `story:tts-contract`: the deterministic run below reads the mock synthesiser and the playback-acknowledging sink double that story creates in `inference-testkit` behind feature `tts`, and no earlier edge of this story reaches it.

## Planned scope

- One pipeline declaration composing the local adapters of local-voice-cascade, the VAD and
  turn-policy nodes (`story:endpoint-controller`), the authority and executor nodes
  (`story:tool-authority-and-executor-nodes`) and the loop (`story:tool-feedback-loop`), with the
  mock executor of the testkit serving one read-only tool that answers a question the model cannot
  answer from its weights (a fixed fact the test controls), plus one `idempotent_write` tool so
  that the audit shows it was never called.
- The LLM node offers the declared tools to its adapter and starts its generation for a turn at
  the `TurnCompleted` of that turn by default (778, 606).
- The result is reinjected, the LLM continues, the commit gate commits the segments, TTS
  synthesizes, the sink acknowledges playback.
- Network denied for the whole run, the tool included (the A-04 test of local-voice-cascade
  reused).
- The deterministic run, which is the acceptance: the same declaration bound to the testkit mocks
  in place of the local adapters. The synthetic audio fixture and the VAD and turn-policy nodes of
  `story:endpoint-controller` stand in for the spoken question; the mock recogniser of
  `story:audio-and-stt-contract` is scripted to emit and commit the question's span under the turn
  the policy node opened; the mock LLM is scripted as in `story:tool-feedback-loop` to propose the
  read-only call and continue with text containing the result; the mock synthesiser and the sink
  double of `story:tts-contract` acknowledge playback; the mock executor serves the two tools. One
  declaration, two sets of bindings: what A-05 (1141) asks of adapters is what makes this run and
  the demonstration below the same pipeline. Test file
  `crates/inference-runtime/tests/cascade_read_only_tool.rs`, beside the A-02 test.
- The run with the real adapters is a documented demonstration, below, not the acceptance.

## Demonstration with the real adapters

A real model's output varies (1002), so this run is not reproducible and gates nothing. It is the
same declaration bound to the local adapters of `epic:local-voice-cascade`, run as an ignored test
beside that epic's A-04 test, with every field below written into this body before the first run
and the result (date, pass or fail, transcript on failure) written after it.

| field | pinned to |
|---|---|
| model | the LLM adapter and checkpoint `epic:local-voice-cascade` selects once `decision-blocker:first-adapters` is decided: adapter crate, checkpoint name and digest |
| sampling | greedy decoding (temperature 0, no nucleus or top-k sampling) and a fixed seed where the adapter exposes one |
| prompt | the system instruction, the question's wording and the tool description, fixed by the test and quoted here |
| audio | a recorded utterance of the question, the replay-fixture kind the testkit holds (244), added by the demonstration, not by the acceptance |
| pass rule | one run with the settings above shows the observables of the acceptance: one Completed read-only call with `input_basis` committed and the generation_id of the played segments, no call for the idempotent-write tool, played text containing the result |
| failure | a run in which the model proposes no call fails the demonstration and is recorded with the transcript and the settings; it is not evidence against the framework, the deterministic run is |

## Domain relations

- Pipeline references DeploymentProfile, one: `ess/domains/graph.yaml`, `inference.graph.Pipeline`,
  relation `profile`; the profile is `offline_strict` with egress `denied`.
- Pipeline owns Node and Edge, many: `ess/domains/graph.yaml`, `inference.graph.Pipeline`,
  relations `nodes` and `edges`.
- ToolCall references Generation and Tool, one each: `ess/domains/tools.yaml`,
  `inference.tools.ToolCall`, relations `generation` and `tool`.
- Generation owns OutputSegment, many: `ess/domains/stream.yaml`, `inference.stream.Generation`,
  relation `segments`; the spoken result is a segment of the generation that proposed the call.
- UNMAPPED: Pipeline to Tool. No `relations:` entry says which declared tools a pipeline offers
  its LLM node; this pipeline offers every declared tool and routes every call to its one executor
  node (769). Settle it with per-tool permission gates (1054) when a pipeline needs an allowlist.

## Acceptance

In the cascade declaration run on the testkit mocks with network denied, the played segments'
committed text contains the tool result, the `inference.tools.ToolCalls` view shows one Completed
call with `input_basis` committed and the generation_id of those segments and no call for the
idempotent-write tool, and the run passes `inference.tools.ProposeToolCall/outcome/proposed`,
`inference.tools.CompleteToolCall/outcome/completed`,
`inference.stream.Turn/transition/complete/by/inference.stream.CompleteTurn/completed` and
`inference.stream.OutputSegment/transition/play/by/inference.stream.AcknowledgePlayback/played`,
with the check below holding.

## Checks

1. The mock LLM adapter records the tool set the LLM node offered it: the read-only tool and the
   idempotent-write tool, both declared through `DeclareTool`.

## Out of scope

A real executor or MCP; a confirmation UI; a side-effecting tool actually executing; tool
execution while the user talks (§15.3); the real-adapter run as an acceptance (it is the
demonstration above).

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). The tree holds no code and no commit (`git log`: "does
not have any commits yet"; `crates/` and the root `Cargo.toml` absent), so every `crates/` path below
is one a body names, not one on disk.

- **Primary surface:** `crates/inference-runtime/tests/cascade_read_only_tool.rs`, new: the one cascade declaration, its two sets of bindings (testkit mocks for the acceptance, local adapters for the demonstration), the scripted mocks and the two-tool table handed to the mock executor — cited (Scope bullet 5: "Test file `crates/inference-runtime/tests/cascade_read_only_tool.rs`, beside the A-02 test"; `docs/design/inference.md:1161`, `:1175`, `:1141`)
- **Files:** the LLM node, which this story edits in two places: its start trigger (generation for a turn starts at `TurnCompleted` by default, Scope bullet 2, `docs/design/inference.md:778`, `:606`) and the tool set it offers its adapter (Scope bullet 2, Check 1) — the surface is cited (Context paragraph 2: "this story and `story:tool-call-interruption` both edit the LLM node (the model node of `story:text-pipeline-end-to-end`, given its tool ports by `story:tool-feedback-loop`)"); no body names its file, and `review-result:tools-parallel-round-1` and `-round-2` both record that
- **Files:** `crates/inference-testkit/src/llm.rs` — inferred: the file the `story:text-pipeline-end-to-end` scope places "the model node wrapping the scripted mock LLM adapter" in, and the file the recorded scope of `story:llm-contract` gives the mock LLM adapter; Check 1 has that mock "record the tool set the LLM node offered it", which no earlier body scripts (`story:tool-feedback-loop` scripts a proposal and a continuation only), so the mock is extended here too; `crates/inference-testkit/src/graph.rs` and a file under `crates/inference-pipeline/src/nodes` are the other readings of where the node sits
- **Also likely:** `crates/inference-runtime/Cargo.toml` — inferred: the test binds the mock synthesiser and the sink double behind testkit feature `tts` (Context paragraph 3) and the mock recogniser and synthetic fixture behind `stt`, and no earlier runtime test names `tts`; the dev-dependency on `inference-testkit` (feature `llm` since the A-02 story's own scope) gains the feature, plus `inference-tts`, `inference-audio` or `inference-tools` dev-dependencies if the test names their types; no body names this edit
- **Symbols:** `inference.graph.Pipeline` relation `profile` to `inference.policy.DeploymentProfile` and relations `nodes`, `edges` (`ess/domains/graph.yaml:88,107-130`); `offline_strict` and egress `denied` (`ess/domains/policy.yaml:24,30`); `inference.tools.ToolCall` fields `generation_id`, `input_basis` and relations `tool`, `generation` (`ess/domains/tools.yaml:71-98`), `DeclareTool` (`:113`), `ProposeToolCall` (`:144`), `ToolCalls` view with `input_basis` (`:219-231`); `inference.tools.CompleteToolCall` and the state Completed, absent today (`tools.yaml:3`) and declared by `story:tool-call-lifecycle`; `inference.stream.Generation` relation `segments` (`ess/domains/stream.yaml:121,148-150`), `OutputSegment` transition `play` (`:196-198`), `CompleteTurn` outcome `completed` emitting `TurnCompleted` (`:255-268`, `:590`), `AcknowledgePlayback` outcome `played` (`:536-549`); design lines 244, 606, 625, 759-778, 866, 1002, 1046, 1054, 1141, 1161, 1175 read as the body says — cited; all consumed, none declared or changed here
- **Read, not changed:** `ess/domains/graph.yaml`, `ess/domains/tools.yaml`, `ess/domains/stream.yaml`, `ess/domains/policy.yaml` (the four Domain relations; the `UNMAPPED` Pipeline-to-Tool relation is deferred by the body to a later allowlist); `crates/inference-pipeline/src/nodes/vad.rs`, `crates/inference-pipeline/src/nodes/turn_policy.rs` (`story:endpoint-controller`), `crates/inference-pipeline/src/nodes/tool_authority.rs`, `crates/inference-pipeline/src/nodes/tool_executor.rs` (`story:tool-authority-and-executor-nodes`) and the feedback bound of `story:tool-feedback-loop`, all composed by the declaration; `crates/inference-testkit/src/mock_tool_executor.rs` (the two tools are its constructor table, authority story Scope bullet 3, not an edit); `crates/inference-testkit/src/stt.rs`, `crates/inference-testkit/src/stt/synthetic_audio.rs`, `crates/inference-testkit/src/tts.rs` (scripted and replayed, Scope bullet 5) — cited, each named by the body only as another story's surface
- **Not touched:** root `Cargo.toml`, `Taskfile.yml`, `AGENTS.md`, `README.md`, `ess/SKIPPED.md`, `crates/inference-runtime/src`, `crates/inference-tools`; the commit-gate node, the acknowledging audio sink, the local adapters and the A-04 test, which the body assigns to `epic:local-voice-cascade` — cited (the body names no change to any of them)
- **Documents:** none in the tree — cited; the story's own body takes the demonstration table before the real-adapter run and the result after it (Demonstration section, through `aep plan artifact body`), and the four scenario ids the Acceptance names are already claimed by `story:tool-call-lifecycle` (`ProposeToolCall/outcome/proposed`, `CompleteToolCall/outcome/completed`), `story:endpoint-controller` (`Turn/transition/complete/...`) and `story:outbound-text-generations-and-segments` (`OutputSegment/transition/play/...`, its id list line 98), so no `ess/SKIPPED.md` line leaves here
- **Confidence:** medium — the one new file is named by the body; the LLM-node edit is a surface the body names in a file no body names, which two critic rounds confirm; the manifest edit is a reading of the feature gates
- **Would collide with:** any unit touching the file the LLM node lives in and the testkit `llm` module (`story:tool-call-interruption` and `story:tool-feedback-loop` by the body's own words, both ordered before this story by `depends_on`); any unit adding a file under `crates/inference-runtime/tests` or a dev-dependency line to `crates/inference-runtime/Cargo.toml` (the A-02, endpoint-controller and authority tests, all ancestors by edge); nothing under `ess/`, `crates/inference-pipeline` or `crates/inference-tools` — cited
- **Safety fact:** every surface but one is new or used as built: the declaration is one new test file composing nodes, mocks and a fixture five earlier stories ship, `ess/` is unchanged so the synthesized suite and the `unsupported == ess/SKIPPED.md` equality of `crates/inference-testkit/tests/conform.rs` cannot move, and the executor's two tools are a constructor table (`story:tool-authority-and-executor-nodes`, Scope bullet 3). The one edit to an existing surface is the LLM node's default start trigger, and walked at level 3 it does not reach cleanly: the body makes `TurnCompleted` the default because "the earlier pipelines are text-only and have no turn to start on" (Context paragraph 2), yet the A-02 model node starts its generation at run start and emits per input envelope (`story:text-pipeline-end-to-end`, Context) and the loop and interruption tests reuse that pipeline, so the default must be conditional on a turn-control input being wired or three earlier test files change — cited, unproven; the implementor proves this first. Where the symbol search stops: whether the `inference-llm` request already carries an offered tool set after `story:tool-feedback-loop`, which Check 1 reads back from the mock — unproven
