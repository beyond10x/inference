---
format: aep.planning-md/3
id: story:pipeline-review-fixes
kind: story
status: implemented
title: 'Pipeline declaration and validation: the six problems of the 0.3.0 review'
relations:
- decomposes: epic:graph-minimum
- serves: vision:inference
- depends_on: story:capability-planning
scope:
- confidence: cited
  path: crates/inference-pipeline
- confidence: cited
  path: crates/inference-testkit/src/graph.rs
- confidence: cited
  path: crates/inference-testkit/src/model.rs
- confidence: cited
  path: crates/inference-testkit/src/policy.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/graph.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/mod.rs
- confidence: cited
  path: ess/SKIPPED.md
- confidence: cited
  path: ess/domains/graph.yaml
revision: 13
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T08:33:19Z", actor: "human:timo", revision: 10}
- {from: "proposed", to: "active", at: "2026-10-19T08:33:19Z", actor: "human:timo", revision: 11}
- {from: "active", to: "implemented", at: "2026-10-19T09:36:13Z", actor: "human:timo", revision: 13, decided_on: {"recorded":{"test_result":1,"review_outcome":3,"verification":1}}}
---
## Context

A read-only review of the changes since 0.3.0 (`review-result:review-030-pipeline`, 2026-10-19)
found six problems in `crates/inference-pipeline` and the testkit's graph target:

1. A forced `ValidatePipeline/rejected` answers before the store's state check
   (`crates/inference-testkit/tests/target/graph.rs:304-306`), so a Validated pipeline is
   "rejected" with no findings where ESS answers `wrong-state`.
2. The typed builder takes any `&DeploymentProfile` and any `&Adapter` (`builder.rs:359-371`,
   `:267-271`); both are public constructors that need no store, so the builder writes a
   pipeline the commands would refuse (`unknown-profile`, `unknown-adapter`), against its own doc
   (`builder.rs:12-14`).
3. The testkit's domains each count identifiers from `…0001` (`src/policy.rs:49`, `src/graph.rs:70`,
   `src/model.rs`), and the target matches on the bare Uuid, so a profile id is accepted where a
   pipeline id is expected.
4. `AddPort` on a Validated pipeline succeeds and leaves it Validated (`declaration.rs:250-257`);
   `ess/domains/graph.yaml:684-690` marks the guard UNMAPPED.
5. Check 6 flags every essential port with no edge, inputs included (`validation.rs:215-227`);
   design line 530 names reachable source/sink paths and unresolved mandatory outputs only.
6. `PipelineState`'s doc skips `Validated` (`lib.rs:266-267`).

## Decisions

- A forced outcome is applied only where the store's own guards would reach that outcome: the
  state and existence checks run first, as ESS orders them; a forced `rejected` on a Declared
  pipeline carries the store's findings.
- The builder takes catalog-checked values: it reads profiles and adapters through the same
  `Catalog` the commands use, by identity, and refuses an unknown one as the commands do.
- Identifiers the testkit issues are distinct across domains (one counter for the whole store, or a
  per-kind prefix), so a wrong-typed id answers not-found.
- Point 4 is decided in the specification: `AddPort` (and `AddNode`, `ConnectPorts`, `BindNode`) on a
  pipeline that is no longer Declared is refused `not-declared`, declared in `ess/domains/graph.yaml`
  with `ess/SKIPPED.md` following the scenario set.
- Check 6 reports unconnected essential outputs and unreachable sinks; an unconnected input is not a
  check-6 finding.
- The `PipelineState` doc names all states.

## Acceptance

`cargo test -p inference-pipeline` and `cargo test -p inference-testkit --all-features` pass with one
case per decision, each red before the fix, and the conform test passes with `failed` and `error` at
0 and unsupported equal to `ess/SKIPPED.md`.

## Out of scope

Checks 4, 7, 8 and 10.

## Forced outcomes and ports

Decided in wave 10 from the unit's measurement:

- Decision 1 is superseded by ESS. The ESS interpreter (ess 0.60) answers a forced external outcome
  in every state and for an unknown pipeline: a refusal moves nothing, so `wrong-state` never comes
  first. Checking state and existence before a forced `rejected` made conform fail
  (`ValidatePipeline/outcome/rejected` expected `rejected`, got `wrong-state`). The target keeps the
  ESS order for `ValidatePipeline` and `PlanPipeline`, documented there and pinned by two cases.
- `AddPort` takes `pipeline_id`; a port belongs to the pipeline it names, and validation check 1
  finds a port whose node is in another pipeline. A guard on `AddPort` that the node belongs to the
  named pipeline breaks suite synthesis (ESS-SYNTH-003, -004), so it is a validation finding.
- Testkit identifiers count in per-domain spaces (first 8 hex digits: stream 0, model 1, policy 2,
  graph 3).
