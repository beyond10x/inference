---
format: aep.planning-md/3
id: story:subgraph-declaration
kind: story
status: active
title: 'Subgraph nodes: a planned pipeline declared as a node of another, its ports checked against the child''s exposures'
summary: Declares a node's subgraph reference and a run's parent in graph.yaml; planning refuses a subgraph node whose ports do not match the child pipeline's exposures.
relations:
- decomposes: epic:session-facade
- serves: vision:inference
- depends_on: story:pipeline-exposed-ports
scope:
- confidence: cited
  path: crates/inference-pipeline/src/builder.rs
- confidence: cited
  path: crates/inference-pipeline/src/declaration.rs
- confidence: cited
  path: crates/inference-pipeline/src/lib.rs
- confidence: cited
  path: crates/inference-pipeline/src/planning.rs
- confidence: cited
  path: crates/inference-pipeline/tests/subgraph.rs
- confidence: cited
  path: crates/inference-runtime/src/run.rs
- confidence: cited
  path: crates/inference-testkit/src/graph.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/graph.rs
- confidence: cited
  path: ess/domains/graph.yaml
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:56:32Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T22:56:32Z", actor: "human:timo", revision: 4}
---
## Context

§10.3 (`docs/design/inference.md:544`): "Subgraph: expose a complete voice frontend or RAG workflow
as a reusable pipeline node, preserving declared ingress/egress contracts"; ADR-003 (1188): typed
multiport nodes form graphs, subgraphs among them. A-14 (1150) asks that reusable subgraphs compose
without losing cancellation and tracing boundaries. The ingress and egress contracts are the child
pipeline's exposures (`story:pipeline-exposed-ports`). Nothing in `ess/domains/graph.yaml` lets a
node stand for a pipeline, and a run has no parent. This story declares both and the planning
check; `story:subgraph-execution` runs it.

Decisions taken on silence:

- A subgraph node is a Node referencing a child pipeline, declared by a new command
  `inference.graph.AddSubgraphNode` (parent pipeline, name, child pipeline; accepting outcome
  `added`). The child must be Planned, as a session's pipeline must be, so only a runnable
  pipeline is embedded. A parent that gains nodes is Declared and so not Planned, which means a
  pipeline can never contain itself, directly or through another: no cycle check is needed, and
  `graph.yaml` says why beside the command.
- `NodeRole` is not extended: a subgraph node takes the role its declarer names and is told a
  subgraph by its reference.
- The node's ports are added with `AddPort` as for any node. Planning refuses a subgraph node
  unless its ports and the child's exposures match one to one by name, direction and modality, with
  a finding naming the node and the port or exposure. Where ess 0.60.0 cannot compare those rows in
  a predicate, the refusal is external and the store decides it, as `PlanPipeline`'s locality
  refusal is.
- A subgraph node needs no binding: the bindings are its child's, and planning takes the child's
  effective capabilities as the node's under the weakest-node rule (444).
- A run created for a subgraph node instance records that instance as its parent, and a run created
  otherwise records none: one child run per subgraph node instance per parent run, driven by the
  instance's lifecycle (`story:subgraph-execution`). How the parent is set (an input of
  `CreateRun` or a command of its own) is whichever keeps the existing `inference.graph.CreateRun`
  scenarios unchanged, and the story body records which.

## Planned scope

- Spec first, `ess/domains/graph.yaml`: the subgraph field and relation on `inference.graph.Node`;
  `AddSubgraphNode` with outcomes, refusals and event; the interface-mismatch refusal of
  `PlanPipeline`; the parent field and relation on `inference.graph.Run`; `read from:` 544, 1150,
  1188, 444. Then `ess specify validate` and `ess verify conform synthesize` as AGENTS.md lists.
- `inference-pipeline`: the command in `src/declaration.rs`, a builder method in `src/builder.rs`,
  the planning check in `src/planning.rs`, the types in `src/lib.rs`; tests in `tests/subgraph.rs`
  (new).
- `inference-runtime`: the parent on `Run` in `src/run.rs`.
- Testkit: the graph store and its target answer the new command and refusal (`src/graph.rs`,
  `tests/target/graph.rs`).

## Domain relations

- Node references Pipeline as its subgraph, zero or one: declared by this story, read from 544 ("a
  reusable pipeline node": one pipeline per node; reusable, so many nodes may reference one
  pipeline).
- Run references NodeInstance as its parent, zero or one: declared by this story, by the decision
  above.
- Run references Pipeline, one, and owns NodeInstance, many: `ess/domains/graph.yaml`,
  `inference.graph.Run`, relations `pipeline`, `instances`.
- Pipeline owns Node, many: `ess/domains/graph.yaml`, `inference.graph.Pipeline`, relation `nodes`;
  its exposures are declared by `story:pipeline-exposed-ports`.

## Acceptance

`ess specify validate --path ess` exits 0, the testkit conform run answers every `inference.graph`
scenario with none unsupported, `inference.graph.AddSubgraphNode/outcome/added` and the
interface-mismatch refusal of `inference.graph.PlanPipeline` among them, and
`crates/inference-pipeline/tests/subgraph.rs` plans a parent pipeline of a source, a subgraph node
over the exposed A-02 text pipeline and a sink, and refuses one whose subgraph node lacks a port for
the child's egress or carries one of another modality, the finding naming the node and the
exposure.

## Out of scope

Running a subgraph (`story:subgraph-execution`); flattening a subgraph into its parent; a
`subgraph` node role; subgraphs in declarative definitions.

## Implementation record

How a run's parent is set: by a command of its own, `inference.graph.NestRun` (inputs `run_id`,
`parent`; outcomes `unknown-parent` and `nested`, an update of the run; event `RunNested`). The
child run is created by `CreateRun` and then nested. `CreateRun` keeps its input; its `created`
outcome also clears the new `parent` field, and its three scenarios carry no other change. A run
no `NestRun` named records no parent.

A second creating command (`CreateChildRun`) was tried first and dropped: ess 0.60.0 synthesis
then arranged the runs of 219 other scenarios through it, which changed the arrangement of every
run-routed `inference.stream` scenario.

Not refused by `NestRun`: a parent instance that is not one of a subgraph node, or whose node
stands for another pipeline than the run's (ESS-SYNTH-003, two references away); a run that left
Created or already has a parent (UNMAPPED in `graph.yaml`: a guard on the run's state beside the
guard on the parent is refused in one command). An unknown run has no declared outcome.

`AddSubgraphNode` leaves `requires` unset: ess 0.60.0 refuses `[]` and `{cleared: true}` on a
List field that is not Optional, so the node holds the empty list. The `Nodes` view does not
publish `subgraph_id` (adding it changes the width an existing view test pins);
`SubgraphNodeAdded` carries it, and `Runs` publishes `parent`.
