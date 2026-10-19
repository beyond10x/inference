---
format: aep.planning-md/3
id: story:pipeline-review-fixes
kind: story
status: draft
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
revision: 9
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
