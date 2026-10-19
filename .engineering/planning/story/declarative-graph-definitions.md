---
format: aep.planning-md/3
id: story:declarative-graph-definitions
kind: story
status: active
title: Declarative graph definitions checked against runtime node descriptors before anything is declared
summary: Declares NodeDescriptor in the graph domain; a JSON definition is checked against the descriptors, then declared through the commands the builder uses, so validation is shared.
relations:
- decomposes: epic:session-facade
- serves: vision:inference
- depends_on: story:pipeline-exposed-ports
scope:
- confidence: cited
  path: crates/inference-pipeline/Cargo.toml
- confidence: cited
  path: crates/inference-pipeline/src/definition.rs
- confidence: cited
  path: crates/inference-pipeline/src/lib.rs
- confidence: cited
  path: crates/inference-pipeline/src/nodes/descriptors.rs
- confidence: cited
  path: crates/inference-pipeline/src/nodes/mod.rs
- confidence: cited
  path: crates/inference-pipeline/tests/definition.rs
- confidence: cited
  path: crates/inference-testkit/src/graph.rs
- confidence: cited
  path: crates/inference-testkit/src/graph/descriptors.rs
- confidence: cited
  path: crates/inference-testkit/tests/definition.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/graph.rs
- confidence: cited
  path: ess/domains/graph.yaml
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:56:31Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T22:56:31Z", actor: "human:timo", revision: 4}
---
## Context

§9.3 (`docs/design/inference.md:486-493`): besides programmatic typed graphs, "declarative graphs:
TOML/YAML/JSON-like specifications are checked against runtime node descriptors before a graph
runs" (491), because codec, sample rate and provider capabilities are often known only from
configuration (493). P4 delivers "configurable graph definitions" (1162), and the Exit of
`epic:session-facade` asks that they validate against runtime node descriptors.
`epic:graph-minimum` left declarative manifests for later (its Scope). Today a pipeline is declared
by the six commands or the typed builder (`crates/inference-pipeline/src/lib.rs:9-18`), and nothing
describes what a node implementation offers, so there is nothing to check a definition against.
§9.2 (473-482) says what a node descriptor MUST declare.

`NodeDescriptor` is a noun no `ess/1` document declares; this story declares it in
`ess/domains/graph.yaml` before any code.

Decisions taken on silence:

- Format: JSON through `serde_json`, already a workspace dependency and a dev-dependency of
  `inference-pipeline`. YAML and TOML each need a new dependency and are a labelled follow-up.
- Open question 4 (1229), how builders and declarative schemas share validation rules: a
  definition becomes the same `Declaration` through the same commands the builder issues, after
  the descriptor check, so §10.2 validation and §8.3 planning are one code path for both.
- A descriptor is published per node implementation kind, named by a kind string, and carries the
  node's role, its required capabilities and its ports with the §9.2 traits `graph.yaml` already
  types on `inference.graph.Port` (direction, modality, essentiality, delivery, incremental, end
  marker, provisional). Encodings, timing and concurrency (477, 480) stay `UNMAPPED`, as on Port.
- A definition node names a descriptor kind and does not restate its ports: the declared ports
  are the descriptor's. Before any command runs, the check refuses an unknown kind, an edge or
  exposure naming a port the descriptor lacks, an edge leaving an input or entering an output,
  and an edge between ports of different modality, each finding naming the node and the port.
- A declared Node keeps no reference to the descriptor it was checked against: the check runs
  before declaration. Recording it is a follow-up for whoever needs to read it back.
- Descriptors exist for the nodes that exist: VAD, turn policy, tool authority and tool executor in
  `inference-pipeline`, and the testkit's text source, text sink and model node, each set in a
  descriptors file of its own so that no node file is touched.

## Planned scope

- Spec first, `ess/domains/graph.yaml`: `inference.graph.NodeDescriptor` with its port
  descriptors, `DeclareNodeDescriptor` (accepting outcome `declared`; refused for a kind already
  declared), its event and a view, `read from:` 473-482 and 491. Then `ess specify validate` and
  `ess verify conform synthesize` as AGENTS.md lists them.
- `crates/inference-pipeline/src/definition.rs` (new): the serde definition (pipeline name, schema
  revision, mode, profile, nodes by kind, edges, exposures, feedback loops, bindings), the check,
  and the declaration through `Declaration`; exported from `src/lib.rs`.
  `src/nodes/descriptors.rs` (new) with one `mod` line in `src/nodes/mod.rs`. `serde_json` moves
  from dev-dependency to dependency in `crates/inference-pipeline/Cargo.toml`; `Cargo.lock`
  already lists it for the package and does not change.
- Testkit: the graph store and its target answer the new command and view (`src/graph.rs`,
  `tests/target/graph.rs`); the testkit nodes' descriptors in `src/graph/descriptors.rs` (new).
- Tests: the check's refusals in `crates/inference-pipeline/tests/definition.rs` (new); the A-02
  pipeline as a definition in `crates/inference-testkit/tests/definition.rs` (new), where the
  testkit descriptors are in reach.

## Domain relations

- NodeDescriptor owns its port descriptors, many: declared by this story, read from 473-474 ("each
  node descriptor MUST declare named input and output ports").
- Pipeline references DeploymentProfile, one: `ess/domains/graph.yaml`, `inference.graph.Pipeline`,
  relation `profile`; a definition names its profile.
- Pipeline owns Node, Edge and Binding, many: `ess/domains/graph.yaml`, `inference.graph.Pipeline`,
  relations `nodes`, `edges`, `bindings`; and its exposures, declared by
  `story:pipeline-exposed-ports`.
- Node to NodeDescriptor: not declared, by the decision above.

## Acceptance

The testkit conform run answers every `inference.graph` scenario with none unsupported,
`inference.graph.DeclareNodeDescriptor/outcome/declared` and its refusal among them, and
`crates/inference-testkit/tests/definition.rs` declares, validates and plans the A-02 text pipeline
from a JSON definition to the same effective capabilities as the same pipeline built with the typed
builder, while `crates/inference-pipeline/tests/definition.rs` shows a definition naming an unknown
kind, a port its descriptor lacks or an edge across modalities refused with a finding naming the
node and the port and the `Declaration` holding no entity it did not hold before.

## Out of scope

YAML and TOML; schema versioning and migration of definitions; descriptors for the STT and TTS
mock nodes of `story:voice-cascade-on-mocks`; a session opened from a definition.
