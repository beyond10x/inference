---
format: aep.planning-md/3
id: epic:graph-minimum
kind: epic
status: draft
title: Graph minimum (P1)
summary: Typed ports and edges, pipeline validation and planning, a single-run runtime with cancellation; a text-only pipeline on mocks.
relations:
- derived_from: architecture-design:inference
- depends_on: epic:contracts-and-testkit
- serves: vision:inference
revision: 3
---
## Context

Stage P1, "graph minimum" (`docs/design/inference.md:1159`): typed ports, edge validation, a small
runtime, a single-run lifecycle. The crates `inference-pipeline` (nodes, typed graph descriptions,
static validation) and `inference-runtime` (execution and operational policies), lines 241-242.
A pipeline is a declaration and becomes runnable only after binding, validation and planning
(line 519); the planner computes effective capabilities from the weakest node or edge and refuses an
unmet mandatory capability with an actionable diagnostic (lines 444, 453); every edge has finite
buffering (line 679); a run moves Created, Validating, Preparing, Active, Draining, Completed with
failure and cancellation explicit (lines 671-673).

The nouns are declared in `ess/domains/graph.yaml` (`executable-system-specification:inference`).
A pipeline references one `inference.policy.DeploymentProfile` (`ess/domains/graph.yaml`, entity
`inference.graph.Pipeline`, relation `profile`; `docs/design/inference.md:516`), so declaring a
profile is a precondition of declaring a pipeline and the `inference.policy` domain
(`ess/domains/policy.yaml`) lands with this epic.

## Outcome

A text-only pipeline of mock nodes is declared, validated, planned and run to completion, cancels
and drains on request, and a capability mismatch is refused before anything runs.

## Scope

- `inference-core`: the `inference.policy` types `ProfileId`, `ProfileKind`, `Egress` and
  `DeploymentProfile` (`ess/domains/policy.yaml`; the ESS artifact places `inference.policy` in
  `inference-core`), and the testkit answering `inference.policy.DeclareProfile` and the view
  `inference.policy.Profiles`, because `inference.graph.Pipeline` references a profile.
- `inference-pipeline`: Pipeline, Node, Port, Edge, Binding (§9.2, §10.1); the §10.2 checks 1 to 6
  and 9; §8.3 effective capabilities; the programmatic typed builder (§9.3, declarative manifests
  later).
- `inference-runtime`: Run and NodeInstance lifecycles (§12.2), bounded edges with the §12.3
  overload policies, cancellation and drain (§12.1), the node instance lifecycle describe, prepare,
  start, exchange, drain or cancel, stop (line 673), extended in `ess/domains/graph.yaml` from its
  one state.

## Exit

- The synthesized suite, executed by the testkit's Rust runner and reported through
  `ess verify conform report --suite --results` (the route contracts-and-testkit establishes),
  passes every `inference.graph` scenario (85 synthesized on 2026-10-18, more once
  `story:node-instance-lifecycle` extends `inference.graph.NodeInstance`), among them `inference.graph.ValidatePipeline/outcome/rejected`,
  `inference.graph.PlanPipeline/outcome/mismatch`, `inference.graph.ConnectPorts/outcome/unbounded`,
  `inference.graph.Edge/invariant/after/inference.graph.ConnectPorts/connected` and
  `inference.graph.Run/transition/cancel/by/inference.graph.CancelRun/cancelled`, and the one
  `inference.policy` scenario, `inference.policy.DeclareProfile/outcome/declared`; `ess/SKIPPED.md`
  names no `inference.graph` or `inference.policy` id.
- A-02 (line 1138): a text-only streaming pipeline on the mock LLM runs end to end.
- A-13 (line 1149): a node requiring `tool_calling` bound to an adapter without it is refused at
  planning with the capability named.
- A Rust property test: effective capabilities equal the intersection over the nodes on every
  required path (line 444), on random graphs.
- §23.3 tests 1, 2, 4 and 7 (lines 1124-1125, 1127, 1130) pass.

## Out of scope

Real adapters; tools; sessions; declarative YAML/TOML manifests (§9.3, later); subgraphs (A-14,
session-facade).
