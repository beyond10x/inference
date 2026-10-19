---
format: aep.planning-md/3
id: story:capability-planning
kind: story
status: implemented
title: 'Plan a pipeline: effective capabilities from the weakest node and the capability-mismatch refusal'
summary: PlanPipeline, §8.3 intersection as a property test, A-13 refusal naming node and capability
relations:
- decomposes: epic:graph-minimum
- depends_on: story:pipeline-validation
- serves: vision:inference
scope:
- confidence: inferred
  path: crates/inference-core
- confidence: cited
  path: crates/inference-pipeline
- confidence: cited
  path: crates/inference-pipeline/Cargo.toml
- confidence: inferred
  path: crates/inference-testkit/src/graph.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/graph.rs
- confidence: cited
  path: ess/SKIPPED.md
- confidence: cited
  path: ess/domains/graph.yaml
revision: 22
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T07:56:59Z", actor: "human:timo", revision: 18}
- {from: "proposed", to: "active", at: "2026-10-19T07:56:59Z", actor: "human:timo", revision: 19}
- {from: "active", to: "implemented", at: "2026-10-19T08:29:22Z", actor: "human:timo", revision: 22, decided_on: {"recorded":{"test_result":1,"review_outcome":6,"verification":1}}}
---
# Story: capability-planning

## Outcome

A consumer who plans a validated pipeline gets its effective capabilities computed from the weakest node on every required path, or `inference.graph.CapabilityMismatch` naming the node and the unmet capability, so a node requiring `tool_calling` bound to an adapter without it is refused before anything runs (A-13, `docs/design/inference.md:1149`).

## Context

Third story of `epic:graph-minimum`; it is §10.2 check 4 (mandatory model, tool and duplex capabilities are satisfied, `docs/design/inference.md:528`). `inference.graph.PlanPipeline` (`ess/domains/graph.yaml`) moves a Pipeline from Validated to Planned and sets `effective_capabilities` (decision D4 of `executable-system-specification:inference`) and `downgrades`, answers `mismatch` with `inference.graph.CapabilityMismatch` when a mandatory capability of a node or of the pipeline is not implemented by its bound adapter, `unbound` with `inference.graph.NodeUnbound` when a node whose role needs an adapter has no Binding, `locality-denied` with `inference.graph.LocalityDenied` when a bound adapter runs where the pipeline's deployment profile does not admit compute (all three `external`), and `wrong-state` otherwise. The rule is §8.3 (`docs/design/inference.md:444`): effective capabilities are computed, never a union of adapter feature lists; the weakest required edge or node decides. On an unmet mandatory capability planning fails with an actionable diagnostic and a downgrade is never silent (line 453). The computation is a Rust property test, the refusal a scenario (`executable-system-specification:inference`).

The computation, in the nouns of the specification:

- a required path is a path from a node with role `source` to a node with role `sink` along edges whose `source` and `sink` ports are both `essential` (`inference.graph.Essentiality`, read from line 481; "required" at line 444);
- a node on a required path that has a Binding contributes the `capabilities` of the `inference.model.Adapter` its binding references; a node without a binding contributes nothing and is transparent (`inference.graph.Node` has `requires`, not a capability list; only `inference.model.Adapter` has `capabilities`);
- `effective_capabilities` is the intersection of the contributions; over no contributions it is the full `inference.model.Capability` set, which the scenario `inference.graph.Pipeline/transition/plan/by/inference.graph.PlanPipeline/planned` requires (an empty pipeline with `required_capabilities: [output_streaming]` plans);
- `unbound` when a node with a non-empty `requires` has no binding, the error naming that node (the reading the specification's `UNMAPPED` note on `PlanPipeline` `unbound` cites from this story);
- `mismatch` when any node has a capability in `requires` that its bound adapter does not list, or when `Pipeline.required_capabilities` is not a subset of the effective set; the diagnostic names the node (none for a capability the pipeline requires as a whole, `node_id` being optional) and the first unmet capability in the declaration order of `inference.model.Capability`;
- a binding whose `adapter_id` matches no declared `inference.model.Adapter` is treated as an adapter with no capabilities; nothing in the design distinguishes an unknown adapter from one that implements nothing. `BindNode` already refuses such an id (`unknown-adapter`, `inference.graph.AdapterNotFound`), so the rule only guards the Rust planner's lookup.

The planner needs the declared adapters to resolve bindings; the runner holds them from `inference.model.DeclareAdapter` (`story:testkit-conformance-target`, reached through `story:pipeline-declaration`), and the Rust planner takes a lookup from `inference.model.AdapterId` to `inference.model.Adapter`.

## Steps

1. Specification: nothing to add. `ess/domains/graph.yaml` already gives `inference.graph.CapabilityMismatch` the fields A-13 wants (`node_id: Optional<inference.graph.NodeId>`, `capability: inference.model.Capability`), declares `inference.graph.NodeUnbound` and `inference.graph.LocalityDenied` with the `PlanPipeline` outcomes `unbound` and `locality-denied`, and gives the Pipeline `downgrades`. Run `ess specify validate --path ess` and `ess verify conform synthesize --path ess --scenarios ess/scenarios --out .engineering/drafts/suite.json` (no refusal beyond the three ESS-SYNTH-001 refusals the authored scenarios cover).
2. Contract: `CapabilityMismatch { node_id, capability }`, `NodeUnbound { node_id }`, and `effective_capabilities` and `downgrades` on the Planned form in `inference-pipeline`.
3. Behaviour: the computation above; `downgrades` is the empty list, because no preference list exists to downgrade from (Out of Scope); the Planned form is the only input the runtime will take (`story:run-lifecycle`).
4. Testkit: the runner answers `PlanPipeline` with `planned`, `wrong-state` and the injected `mismatch`, `unbound` and `locality-denied`; `ess/SKIPPED.md` loses exactly the nine ids named under Acceptance (`story:testkit-conformance-target`, step 3).
5. Tests: A-13 as a Rust test (a node with `requires: [tool_calling]` bound to a mock adapter whose capabilities lack it is refused, the error naming the node and `tool_calling`); a test that a node with `requires: [tool_calling]` and no binding is refused as `NodeUnbound` naming it; a property test (proptest, dev-dependency) over random acyclic graphs with random bindings asserting effective capabilities equal the intersection over the bound nodes on every required path (epic exit; line 444); a test that an unbound transform between two bound nodes leaves the intersection unchanged.

## Domain relations

All `inferable`:

- `Binding → Adapter`, references one: `ess/domains/graph.yaml`, entity `inference.graph.Binding`, relation `adapter`; the capabilities come from `ess/domains/model.yaml`, entity `inference.model.Adapter`, field `capabilities`.
- `Node → Binding`, zero or one at planning: `inference.graph.PlanPipeline` outcome `mismatch` speaks of its bound adapter, singular; the roles that need none are read from `docs/design/inference.md:100` and `461-469`.
- `Pipeline → effective_capabilities`: a field cleared at `DeclarePipeline` and generated at `PlanPipeline` (`ess/domains/graph.yaml`, outcomes `declared` and `planned`; D4).

## Acceptance

The planner passes `inference.graph.PlanPipeline/outcome/mismatch`, `inference.graph.PlanPipeline/outcome/unbound`, `inference.graph.PlanPipeline/outcome/locality-denied`, `inference.graph.PlanPipeline/outcome/planned`, `inference.graph.PlanPipeline/outcome/wrong-state`, `inference.graph.Pipeline/transition/plan/by/inference.graph.PlanPipeline/planned`, `inference.graph.Pipeline/state/Declared/refuses/inference.graph.PlanPipeline`, `inference.graph.Pipeline/state/Planned/refuses/inference.graph.PlanPipeline` and `inference.graph.Pipeline/state/Planned/refuses/inference.graph.ValidatePipeline` (today `unsupported`, each named in `ess/SKIPPED.md`) with none of the nine left in that file, the A-13 Rust test refuses the `tool_calling` node naming the node and the capability, the unbound node is refused as `NodeUnbound` naming it, and the property test over random graphs holds for the intersection rule.

## Out of Scope

Optional downgrades visible to the application (line 453, second sentence) beyond refusing: `inference.graph.Pipeline.downgrades` records them, but no preference list exists on `inference.graph.Pipeline` yet (optional preferences, design 513, are `UNMAPPED` in graph.yaml), so it stays empty here; planner-suggested transforms (`docs/design/inference.md:497`); the Rust check of locality against the profile (check 8), of which this story delivers only the runner's injected `locality-denied`.

## Ambiguities

- `inferable`: an empty pipeline plans, by the scenario named above; the full set is the identity of the intersection.
- Decisions taken here, each reversible in one file: `downgrades` left empty (step 3); the runner's injected `unbound` and `locality-denied` (step 4); an unknown adapter implements nothing; the required-path reading of line 444 through `essential` ports.

## Open Questions

None.

## Scope

Derived 2026-10-19 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-pipeline` — cited; steps 2, 3 and 5 put the contract, the planner and its tests "in `inference-pipeline`", the epic Scope places §8.3 there, and `story:pipeline-declaration` Context creates it at `crates/inference-pipeline` (`docs/design/inference.md:199`)
- **Files:** `ess/domains/graph.yaml:466` (`inference.graph.CapabilityMismatch`, step 1 adds `node_id` and `capability`) — cited
- **Files:** `ess/SKIPPED.md`, `## graph` section (step 4 removes the seven acceptance ids; file not yet in the tree, created by `story:testkit-conformance-target` step 3) — cited
- **Files:** `crates/inference-pipeline/Cargo.toml` (step 5: `proptest` as dev-dependency) — cited
- **Files:** `crates/inference-testkit/tests/target/graph.rs` (step 4: the runner answers `PlanPipeline` with `planned`, `wrong-state`, injected `mismatch`; the graph-domain target file per `story:testkit-conformance-target` step 2 and `story:pipeline-declaration` step 5) — cited
- **Symbols:** `inference.graph.PlanPipeline` (`graph.yaml:951`), `inference.graph.CapabilityMismatch` (`:466`), `Pipeline.effective_capabilities` (`:157`, cleared `:612`, generated `:1006`), `Node.requires` (`:222`), `Binding.adapter_id` (`:327`), `inference.model.Adapter.capabilities` (`model.yaml:138`), `inference.model.Capability` (`model.yaml:28`, nine variants) — cited
- **Also likely:** `crates/inference-testkit/src/graph.rs` — inferred; the graph store module, if the Validated-to-Planned move and the stored `effective_capabilities` live in the store rather than the target file
- **Also likely:** `crates/inference-core` — inferred; only if `Capability` lacks a declaration-order iteration or `Ord`, which "first unmet capability in declaration order" needs and `story:workspace-skeleton`'s Checks do not require
- **Not a surface:** root `Cargo.toml` (members glob; no story of this epic edits it, `story:workspace-skeleton` step 1), `Taskfile.yml`, `AGENTS.md`, `README.md`, `crates/inference-runtime` (the Planned form is consumed by `story:run-lifecycle`), `crates/inference-testkit/Cargo.toml` (the `inference-pipeline` dependency is `story:pipeline-declaration`'s) — cited
- **Scratch, no entry:** `.engineering/drafts/suite.json` is re-synthesized in step 1 but git-ignored (`.gitignore`: `/.engineering/drafts/`), so it cannot merge-conflict — cited
- **Documents:** none changed; `docs/design/inference.md` is read at 100, 444-453, 461-469, 481, 497, 528, 1149 only — cited
- **Confidence:** high — the body names the crate, the spec line and the skipped-list; the two testkit paths are read from the neighbour stories' layout, not from a tree that does not exist yet
- **Would collide with:** any unit editing `ess/domains/graph.yaml` lines 430-579 (the `errors:` block: `IncompatibleGraph` at 456 and `CapabilityMismatch` at 466 are one diff hunk), the `## graph` section of `ess/SKIPPED.md`, `crates/inference-testkit/tests/target/graph.rs` or `src/graph.rs`, or anything in `crates/inference-pipeline`
- **Safety fact:** `inference.graph.CapabilityMismatch` is referenced by exactly one outcome (`PlanPipeline/mismatch`, `graph.yaml:995`) and the synthesized `mismatch` scenario asserts the error by name only (`expect_error`, no field assertion; `.engineering/drafts/suite.json`, 153 scenarios), and `effective_capabilities` is written by two outcomes (`:612`, `:1006`) and read by one view (`inference.graph.Pipelines`, `:1431`), so the two new fields and the computation reach no other command — step 2 (`grep -n` over `ess/` and the suite), unproven; the search stops at whether ess 0.60's synthesizer emits field assertions once the error has fields, which step 1's re-synthesis (0 refusals) is the check for

## Validated builder form

Added 2026-10-19 from wave 8 (`review-result:adversary-pipeline-validation-pass-1`, acceptance note).

`story:pipeline-validation` step 3 asked for "the Validated form of the typed builder" as the return value; it was not built: `inference_pipeline::Builder::build` (`crates/inference-pipeline/src/builder.rs`) still returns the bare pipeline identity, and validation is a separate call by that identity. Planning is the first consumer that needs a validated pipeline, so this story gives the typed builder a validated form: a handle that only `ValidatePipeline`'s `validated` outcome produces, and that planning takes, so planning an unvalidated pipeline does not compile through the builder.

## No required path

Decided in wave 9 after adversary pass 1:

- When the pipeline has no essential source-to-sink path, every bound node contributes its
  adapters to the intersection. A capability the pipeline requires and a contributing adapter
  lacks is refused `CapabilityMismatch` with no node, as `ess/domains/graph.yaml` words the
  `mismatch` outcome ("a mandatory capability of a node or of the pipeline is not implemented by
  its bound adapter").
- A pipeline with no node has no contributing adapter, so nothing falls short and it plans, with
  an empty effective set: no capability is claimed that no adapter implements. The conform
  scenario `PlanPipeline/outcome/planned` plans such a pipeline.
- A forced planning outcome in the testkit target carries every non-Optional field of its
  declared error: the planner's own finding where it has one, otherwise a deterministic value
  (the first declared node; the first capability the pipeline requires, else the first of
  `Capability::ALL`). `locality-denied` likewise.
