---
format: aep.planning-md/3
id: story:session-topology-switch
kind: story
status: implemented
title: One session switches between the text-only pipeline and the voice cascade with one event vocabulary (A-12), and describe reports what runs
summary: Declares SwitchPipeline in session.yaml; the live session drains its run, attaches the other pipeline's run and keeps its history and vocabulary; describe reports pipeline, bindings, capabilities and downgrades.
relations:
- decomposes: epic:session-facade
- serves: vision:inference
- depends_on: story:session-history
- depends_on: story:voice-cascade-on-mocks
- depends_on: story:session-live-text
scope:
- confidence: inferred
  path: crates/inference-facade/Cargo.toml
- confidence: cited
  path: crates/inference-facade/src/describe.rs
- confidence: cited
  path: crates/inference-facade/src/lib.rs
- confidence: inferred
  path: crates/inference-facade/src/live.rs
- confidence: cited
  path: crates/inference-facade/src/session.rs
- confidence: cited
  path: crates/inference-facade/src/switch.rs
- confidence: cited
  path: crates/inference-facade/tests/topology_switch.rs
- confidence: cited
  path: crates/inference-testkit/src/cascade.rs
- confidence: cited
  path: crates/inference-testkit/src/session.rs
- confidence: cited
  path: crates/inference-testkit/tests/target/session.rs
- confidence: cited
  path: ess/domains/session.yaml
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T22:56:34Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-19T22:56:34Z", actor: "human:timo", revision: 5}
- {from: "active", to: "implemented", at: "2026-10-20T00:06:55Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

A-12 (`docs/design/inference.md:1148`): "an application-facing session can switch among supported
topologies without changing its public event vocabulary"; the P4 exit (1162): "clients switch
text-only / voice-cascade configuration transparently". `ess/domains/session.yaml:67-72` leaves the
switch `UNMAPPED`: "the session holds one pipeline for its lifetime ... whether a switch drains the
current run, and whether a session may replace a current run that is not terminal, is the
topology-switch question; AttachRun below replaces the current run whatever its state". describe
(718) inspects "selected models, graph, capabilities, and degradation" and is not declared either.
Transport persistence is not model persistence (731): the session stays while runs come and go.

The text-only side is `story:session-live-text`; the voice cascade on mocks is
`story:voice-cascade-on-mocks`; the history that has to survive the switch is
`story:session-history`.

Decisions taken on silence:

- Switching is a session command, `inference.session.SwitchPipeline` (session id, pipeline id,
  mode), from Ready only, refused as `OpenSession` refuses an unknown pipeline, one not Planned and a
  mode the pipeline does not declare. Its accepting outcome, `switched`, sets the session's
  pipeline and mode, clears its current run and moves it to Opened through a transition `switch`,
  so readiness is asserted again for the next run, as `attach` does. `Session.pipeline` stays
  cardinality one: one pipeline at a time.
- The run below is drained, not cancelled: the facade sends `stop` into every ingress of the old
  run, waits for Completed, then creates, attaches and activates the new pipeline's run and marks
  the session ready. Input sent meanwhile is refused by the Ready guard of `SendInput`.
- `AttachRun` keeps replacing the current run whatever its state (`session.yaml:70-72`, a stated
  deviation): the switch never reaches it with a run that is not terminal, because the facade
  drains first, and no refusal is added.
- The vocabulary is the one `SessionEvent` of `story:session-live-text`; the cascade's payload
  implements the same conversion trait. The new run is announced by `RunAttached` (339 names no
  switch event).
- History stays on the session across the switch; the new run's model does not receive it
  (`story:session-history`).
- describe answers at the call the session's pipeline id and name, its interaction mode, its nodes
  with their bindings and adapters, the planned pipeline's effective capabilities and downgrades,
  and its exposures. It reads the graph and is not a session command.
- Both pipelines are built with the typed builder; opening a session from a declarative definition
  is not part of this story.

## Planned scope

- Spec first, `ess/domains/session.yaml`: `SwitchPipeline` with its outcomes, refusals, event and
  the `switch` transition, `read from:` 1148, 1162, 731, 713; the marker at 67 goes, and the one at
  158 is replaced by a note that receive and describe are reads. Then `ess specify validate` and
  `ess verify conform synthesize` as AGENTS.md lists.
- `crates/inference-facade`: the command in `src/session.rs`; `src/switch.rs` and `src/describe.rs`
  (new); `src/lib.rs`.
- Testkit: the session store and its target answer the new command (`src/session.rs`,
  `tests/target/session.rs`); the cascade payload implements the facade trait in `src/cascade.rs`.
- Test `crates/inference-facade/tests/topology_switch.rs` (new).

## Domain relations

- Session references Pipeline, one, now one at a time, and Run, zero or one:
  `ess/domains/session.yaml`, `inference.session.Session`, relations `pipeline`, `run`.
- Pipeline owns Binding, many; Binding references Node and Adapter, one each:
  `ess/domains/graph.yaml`, `inference.graph.Pipeline` relation `bindings`,
  `inference.graph.Binding` relations `node`, `adapter`; describe reads them, with the Pipeline
  fields `effective_capabilities` and `downgrades`.
- HistoryEntry belongs to Session: declared by `story:session-history`.
- UNMAPPED, left so: a run current in at most one session (`session.yaml:73-81`), decided but not
  declarable under ess 0.60.0 (ESS-SYNTH-001); a retried attach of the run already held
  (`session.yaml:91-95`, ESS-SYNTH-003). Neither is reached by the switch.

## Acceptance

The testkit conform run answers every `inference.session` scenario with none unsupported,
`inference.session.SwitchPipeline/outcome/switched` and its refusals among them, and
`crates/inference-facade/tests/topology_switch.rs` runs one receiving function, written once against
`SessionEvent` with no branch on topology, over a session opened on the text-only pipeline, switched
to the voice cascade on mocks and fed the synthetic utterance, then switched back, where each phase
completes an exchange, the old run is Completed before the new one is attached, the history holds
the entries of all three phases in order, and describe names a different pipeline and different
effective capabilities in the voice phase than in the text phases.

## Out of scope

Switching by cancelling a run; switching while Draining; a session opened from a definition;
replaying history into the new run's model; transports beyond in-process calls.
