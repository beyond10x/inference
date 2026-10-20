---
format: aep.planning-md/3
id: story:pipeline-exposed-ports
kind: story
status: implemented
title: A pipeline exposes named ingress and egress ports, and its run hands their outer ends to the caller
summary: Declares the named source and sink interfaces of design line 511 as exposures; validation counts them as connected and the run controller builds a bounded queue for each, whose far end its caller holds.
relations:
- decomposes: epic:session-facade
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-pipeline/src/builder.rs
- confidence: cited
  path: crates/inference-pipeline/src/declaration.rs
- confidence: cited
  path: crates/inference-pipeline/src/lib.rs
- confidence: cited
  path: crates/inference-pipeline/src/validation.rs
- confidence: cited
  path: crates/inference-pipeline/tests/exposure.rs
- confidence: cited
  path: crates/inference-runtime/src/controller.rs
- confidence: cited
  path: crates/inference-runtime/src/drive.rs
- confidence: inferred
  path: crates/inference-runtime/src/lib.rs
- confidence: cited
  path: crates/inference-runtime/tests/exposed_ports.rs
- confidence: cited
  path: crates/inference-testkit/src/graph.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/graph.rs
- confidence: cited
  path: ess/domains/graph.yaml
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:56:31Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-19T22:56:31Z", actor: "human:timo", revision: 5}
- {from: "active", to: "implemented", at: "2026-10-20T00:06:54Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

A pipeline declaration SHOULD include "named source and sink interfaces exposed to the application"
(`docs/design/inference.md:511`); `ess/domains/graph.yaml:168-170` leaves them `UNMAPPED` ("the
design lists them in the declaration without saying their shape"). Two parts of
`epic:session-facade` need them: the facade sends and receives through a pipeline without knowing
its topology (107, 713-715), and a subgraph exposes a pipeline as a node "preserving declared
ingress/egress contracts" (544). Today the only way into a run is a source node the test drives
(`TextSource` and its `SourceHandle`, `crates/inference-testkit/src/graph/nodes.rs`), and
`RunController` builds queues for declared edges only (`crates/inference-runtime/src/lib.rs:14-19`).

Decisions taken on silence:

- An exposure names one port of the pipeline, a name, and the finite capacity and overload policy
  every edge MUST specify (679): it is an edge whose far end is outside the pipeline. Its
  direction is the port's: an input port is an ingress, an output port an egress. It is declared
  by a new command, `inference.graph.ExposePort`, on a Declared pipeline, refused for an unknown
  pipeline, an unknown port, a port of another pipeline and an unbounded capacity, as
  `inference.graph.ConnectPorts` refuses them; its accepting outcome is `exposed`.
- Two exposures of one name in one pipeline are a validation finding (a new `GraphDefect`
  variant), not a command refusal, as node-name uniqueness is not refused by `AddNode` either
  (`graph.yaml:727`).
- §10.2 check 6 (530, reachable source and sink paths) counts an exposed input port as fed and an
  exposed output port as consumed.
- At run time the end of input of an ingress is the `stop` control event its caller sends on it,
  as a source node's is today (`crates/inference-runtime/src/drive.rs:4-8`): the caller is the
  source.
- The typed builder gains `expose`, carrying the port's direction and modality as types, as
  `connect` does.

## Planned scope

- Spec first, `ess/domains/graph.yaml`: the exposure entity and its relations, `ExposePort` with
  its outcomes, refusals and event, a view, the `GraphDefect` variant, each with `read from:`
  lines (511, 544, 679, 530). The 511 `UNMAPPED` marker on `inference.graph.Pipeline` loses its
  source and sink part and keeps tool permissions, resource budgets and preferences. Then
  `ess specify validate --path ess` and `ess verify conform synthesize` with no refusal beyond the
  three ESS-SYNTH-001 ones AGENTS.md allows.
- `inference-pipeline`: the command in `src/declaration.rs`, the builder method in
  `src/builder.rs`, check 6 and the duplicate finding in `src/validation.rs`, the types in
  `src/lib.rs`; tests in `tests/exposure.rs` (new).
- `inference-runtime`: `src/controller.rs` builds one `EdgeQueue` per exposure, hands the node the
  inner end in its `NodeEndpoints` and the caller the outer ends by exposure name;
  `src/drive.rs` counts the caller's `stop` on an ingress as end of input. Test
  `tests/exposed_ports.rs` (new).
- Testkit: the graph store and its target answer the new command and view
  (`crates/inference-testkit/src/graph.rs`, `crates/inference-testkit/tests/target/graph.rs`).
  `tests/target/mod.rs` routes by domain (`mod.rs:268-318`) and does not change.

## Domain relations

- Pipeline owns Node, Edge and Binding, many: `ess/domains/graph.yaml`, `inference.graph.Pipeline`,
  relations `nodes`, `edges`, `bindings`.
- Node owns Port, many: `ess/domains/graph.yaml`, `inference.graph.Node`, relation `ports`.
- Port references Pipeline, one: `ess/domains/graph.yaml`, `inference.graph.Port`, relation
  `pipeline`.
- Pipeline owns its exposures, many: declared by this story, read from 511 ("interfaces", plural),
  with the ownership `edges` has, so an exposure exists only within its declaration.
- An exposure references one Port: declared by this story (an exposure names one port).
- UNMAPPED, left so: whether one port may carry more than one exposure. The design does not say;
  `ExposePort` does not refuse it and the marker stays in `graph.yaml`.

## Acceptance

`ess specify validate --path ess` exits 0 with the source and sink part of the 511 marker gone, the
testkit conform run answers every `inference.graph` scenario with none unsupported,
`inference.graph.ExposePort/outcome/exposed` and the refusals the command declares among them, and
`crates/inference-runtime/tests/exposed_ports.rs` runs the A-02 model node between an exposed
ingress and an exposed egress with no source or sink node, the test sending text and `stop` into
the ingress and reading the reply from the egress until the run is Completed and every queue is
dropped.

## Out of scope

The session that uses exposures (`story:session-live-text`); subgraphs
(`story:subgraph-declaration`); declarative definitions (`story:declarative-graph-definitions`);
tool permissions, resource budgets and preferences, which stay in the marker.
