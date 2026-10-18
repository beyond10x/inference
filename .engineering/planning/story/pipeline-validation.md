---
format: aep.planning-md/3
id: story:pipeline-validation
kind: story
status: draft
title: 'Validate a pipeline: the structural checks of §10.2 refuse an incompatible graph before it runs'
summary: ValidatePipeline with checks 1, 2, 3, 5, 6 and 9 and IncompatibleGraph findings; §23.3 tests 1 and 2
relations:
- decomposes: epic:graph-minimum
- depends_on: story:pipeline-declaration
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-pipeline
- confidence: inferred
  path: crates/inference-testkit/src/graph.rs
- confidence: inferred
  path: crates/inference-testkit/tests/target/graph.rs
- confidence: inferred
  path: crates/inference-testkit/tests/target/mod.rs
- confidence: cited
  path: ess/SKIPPED.md
- confidence: cited
  path: ess/domains/graph.yaml
revision: 9
---
# Story: pipeline-validation

## Outcome

A consumer who validates a declared pipeline either gets it back Validated or gets `inference.graph.IncompatibleGraph` listing which of the structural checks of §10.2 failed and on which node, port or edge, before anything runs.

## Context

Second story of `epic:graph-minimum`. `inference.graph.ValidatePipeline` (`ess/domains/graph.yaml`) moves a Pipeline from Declared to Validated or answers `rejected` with `inference.graph.IncompatibleGraph`; from any other state it answers `wrong-state` with `inference.graph.PipelineStateConflict`. The checks are §10.2 of the design (`docs/design/inference.md:521-534`); this epic owns checks 1, 2, 3, 5, 6 and 9 (epic scope). Check 4 is planning (`story:capability-planning`). The ESS artifact says the checks are Rust tests and the scenario checks the refusal (`executable-system-specification:inference`, "Not expressed in ESS, checked elsewhere"); the `rejected` outcome is declared `external`, so its scenario injects it and the runner must support the injected outcome.

The checks, in the nouns of graph.yaml:

1. Existence (line 525): every `from_port` and `to_port` of an edge is a port of a node of this pipeline (chain `Pipeline.nodes`, `Node.ports`); every `Binding.node_id` is a node of this pipeline.
2. Direction and type (line 526): `Edge.source` has `direction: output`, `Edge.sink` has `direction: input`, and the two ports share `modality`.
3. Commitment semantics (line 527): a source port with `provisional_allowed: true` may only feed a sink port with `provisional_allowed: true` (`docs/design/inference.md:608`, a model that cannot handle edits receives committed spans only); a source port with `incremental: true` may only feed a sink port with `incremental: true` (`docs/design/inference.md:450`). Media encoding and timebase arrive with `inference-audio` and are not checked here.
5. Cycles (line 529, line 546): every cycle is refused. No noun for a bounded feedback declaration exists in graph.yaml (`docs/design/inference.md:517` lists it as optional); unbounded cycles are invalid by default (line 546). `story:tool-feedback-loop` (`epic:tools-and-turns`) declares the feedback-bound noun in `ess/domains/graph.yaml` when the LLM-to-tool loop needs it and amends check 5 to admit a cycle that carries it; that story depends on this one, and this story claims no such noun.
6. Reachability (line 530): every port with `essentiality: essential` is connected by at least one edge; every node with role `sink` is reachable from a node with role `source`; `optional` and `control_only` ports may be unconnected.
9. Tool gating (line 533): a node with role `tool` gets `provisional_allowed: false` on its input ports by default in the typed builder (`docs/design/inference.md:625`, side-effecting tools require committed input by default), so check 3 refuses provisional text into it; a consumer who sets `provisional_allowed: true` on such a port has made the explicit configuration of line 624. The call-level barrier stays in `ess/domains/tools.yaml` (`inference.tools.ProvisionalInputBarrier`) and `epic:tools-and-turns`.

Every check is vacuous over an empty pipeline: `inference.graph.Pipeline/transition/validate/by/inference.graph.ValidatePipeline/validated` validates a pipeline with no nodes.

## Steps

1. Specification first: `inference.graph.IncompatibleGraph` has no fields and its summary says the diagnostics say why. Add a field `findings: List<String>` to that error in `ess/domains/graph.yaml` so the reason travels with the error, run `ess specify validate --path ess` and `ess verify conform synthesize --path ess --out .engineering/drafts/suite.json` (0 refusals), and say in the PR that this step came first.
2. Contract: `IncompatibleGraph { findings }` in `inference-pipeline`, one finding per failed check naming the check number and the node, port or edge id.
3. Behaviour: `ValidatePipeline` runs checks 1, 2, 3, 5, 6 and 9 over the declaration and moves the pipeline to Validated only when all pass; the Validated form of the typed builder is the return value.
4. Testkit: the runner answers `ValidatePipeline` with `validated`, `wrong-state` and the injected `rejected`; `ess/SKIPPED.md` loses exactly the five ids named under Acceptance (`story:testkit-conformance-target`, step 3).
5. Tests: one Rust test per check with a minimal failing graph and the finding it produces (§23.3 test 1, incompatible ports before execution, and test 2, unbounded cycles and disallowed data flows: `docs/design/inference.md:1124-1125`), plus one passing chain source to transform to sink.

## Domain relations

All `inferable`, from `ess/domains/graph.yaml`:

- `Edge → Port` (`source`, `sink`) resolve inside the owning pipeline: `Pipeline.nodes` owns `Node`, `Node.ports` owns `Port`, `Edge.pipeline_id` names the pipeline; an edge naming a port outside that chain fails check 1.
- `Edge → Port` direction: the design names an edge as connecting an output port to a compatible input port (`docs/design/inference.md:102`; graph.yaml `inference.graph.Edge` summary).
- `Port → Edge`: an `essential` port has one or more edges, other ports zero or more (`inference.graph.Essentiality`, read from line 481; line 530).
- `Binding → Node` resolves inside the owning pipeline (`Binding.pipeline_id`, relation `node`).

## Acceptance

The validator passes the scenarios `inference.graph.ValidatePipeline/outcome/rejected`, `inference.graph.ValidatePipeline/outcome/validated`, `inference.graph.ValidatePipeline/outcome/wrong-state`, `inference.graph.Pipeline/transition/validate/by/inference.graph.ValidatePipeline/validated` and `inference.graph.Pipeline/state/Validated/refuses/inference.graph.ValidatePipeline` (today `unsupported`, each named in `ess/SKIPPED.md`) with none of the five left in that file, and the Rust tests for checks 1, 2, 3, 5, 6 and 9 each refuse their minimal failing graph with a finding naming the check and the offending id while the chain source to transform to sink validates.

## Out of Scope

Check 4 (`story:capability-planning`); checks 7 (resources), 8 (locality against the profile; the `ess/domains/policy.yaml` summary says the planner refuses it, owned by the epic that proves A-04) and 10; bounded feedback declarations (`story:tool-feedback-loop`); media format and timebase compatibility.

## Ambiguities

- `inferable`: the scenario `inference.graph.Pipeline/state/Planned/refuses/inference.graph.ValidatePipeline` needs a Planned pipeline and belongs to `story:capability-planning`.
- `inferable`: an unconnected `optional` port is allowed by check 6 because `inference.graph.Essentiality` distinguishes it from `essential` (line 481); nothing else is read into the enum.
- Decision taken here, reversible in `ess/domains/graph.yaml`: the `findings` field on `inference.graph.IncompatibleGraph` (step 1), so that line 453 (actionable diagnostics) holds on the wire.

## Open Questions

None.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-pipeline` — cited: steps 2 and 3 land `IncompatibleGraph { findings }` and `ValidatePipeline` in `inference-pipeline`; the path is the one `story:pipeline-declaration` (Context) creates under the `crates/*` glob of `story:workspace-skeleton` (step 1). The crate does not exist yet.
- **Files:** `ess/domains/graph.yaml` — cited: step 1 adds `findings: List<String>` to `inference.graph.IncompatibleGraph` (declared at line 367, raised at line 565).
- **Files:** `ess/SKIPPED.md` — cited: step 4 and Acceptance remove exactly five `inference.graph` ids. The file does not exist yet; `story:testkit-conformance-target` step 3 creates it with one `## graph` section.
- **Symbols:** `inference.graph.ValidatePipeline` (`ess/domains/graph.yaml:558-584`), `inference.graph.IncompatibleGraph` (`:367`, `:565`), `inference.graph.PipelineStateConflict` (`:374`), `inference.graph.Essentiality` (`:59`), `Port.provisional_allowed` (`:202`), `incremental` (`:478`) — cited. No Rust symbol exists yet; no `.rs` file is in the tree.
- **Also likely:** `crates/inference-testkit/tests/target/graph.rs` — inferred: step 4 says the runner answers `ValidatePipeline` with `validated`, `wrong-state` and the injected `rejected`; the per-domain target file is where `story:testkit-conformance-target` (step 2) and `story:pipeline-declaration` (step 5) put every `inference.graph` answer.
- **Also likely:** `crates/inference-testkit/src/graph.rs` — inferred: the `graph` store module of the same split holds the pipeline state the Declared to Validated move changes.
- **Also likely:** `crates/inference-testkit/tests/target/mod.rs` — inferred: the `rejected` scenario opens with `configure_external_outcome` / `force` (`suite.json`, `inference.graph.ValidatePipeline/outcome/rejected`), the first of only two such steps in the system (the other is `PlanPipeline/mismatch`); if `ess_conformance` routes a forced outcome through the `ConformanceTarget` impl rather than inside its `Runner`, the shared dispatch changes here, and `story:testkit-conformance-target` step 2 does not mention that step.
- **Documents:** `ess/domains/graph.yaml` and `ess/SKIPPED.md`, both above. `docs/design/inference.md` is read, not changed: lines 102, 450, 453, 481, 517, 521-534, 546, 608, 624-625 and 1124-1125 all say what the story quotes — cited.
- **Not this story's:** `ess/domains/tools.yaml` and `ess/domains/policy.yaml` (named as other epics' surfaces), root `Cargo.toml` (glob members; `story:workspace-skeleton` step 1 says no story of this epic edits the list), `Taskfile.yml`, `AGENTS.md`, `README.md` (no step changes them), `.engineering/drafts/suite.json` (regenerated at test time and git-ignored) — cited.
- **Confidence:** medium — the story names the crate and both documents, but no code exists, so every path under `crates/` is read from the layout its dependencies promise rather than from a tree, and the injected-outcome mechanism is unplaced.
- **Would collide with:** any unit touching the validation half of `crates/inference-pipeline`; the `graph` domain files of `crates/inference-testkit` (`tests/target/graph.rs`, `src/graph.rs`); the `## graph` section of `ess/SKIPPED.md`; the errors and `ValidatePipeline` blocks of `ess/domains/graph.yaml`; and, if injection goes through it, `crates/inference-testkit/tests/target/mod.rs`, which every testkit story shares — inferred.
- **Safety fact:** `inference.graph.IncompatibleGraph` is declared once (`ess/domains/graph.yaml:367`) and raised by exactly one outcome, `ValidatePipeline/rejected` (`ess/domains/graph.yaml:565`), and no other file under `ess/` names it, so adding `findings` changes one command's error contract and no other command, event or domain — step 2 (`rg -n IncompatibleGraph ess/`, 2 hits), unproven. Where the search stops: the synthesized `rejected` scenario asserts only `expect_error: inference.graph.IncompatibleGraph` with no field today; whether synthesize emits a payload assertion for `findings` is what step 1's 0-refusals run proves — cited.
