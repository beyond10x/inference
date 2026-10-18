---
format: aep.planning-md/3
id: epic:session-facade
kind: epic
status: draft
title: Session facade and reusable subgraphs (P4)
summary: One session interface over text-only and voice topologies; subgraphs compose without losing cancellation or tracing.
relations:
- derived_from: architecture-design:inference
- depends_on: epic:tools-and-turns
- serves: vision:inference
revision: 2
---
## Context

Stage P4, "reusable subgraphs and facade" (`docs/design/inference.md:1162`): the session API,
configurable graph definitions, replay and telemetry; exit "clients switch text-only / voice-cascade
configuration transparently". The facade is optional and sits above graph executions (§13, lines
703-743; ADR-005, line 1190): open, send, receive, commit input, interrupt, describe, close (lines
713-719); three states kept apart, application, model inference and media (lines 737-741).
Subgraphs expose a whole voice frontend as one node with its ingress and egress contracts (§10.3,
line 544).

Declared with one state in `ess/domains/session.yaml`; the Session to Run relation is `UNMAPPED`
there and is settled before this epic's first story.

## Outcome

An application opens one session over either the text-only pipeline or the voice cascade and sees
one event vocabulary; a subgraph composes into a parent graph without losing cancellation or
tracing boundaries.

## Exit

- A-12 (line 1148) and A-14 (line 1150).
- The §6.4 session events (line 339) and the lifecycle Opened, Ready, Draining, Closed added to the
  specification pass their scenarios.
- Declarative graph definitions (§9.3) validate against runtime node descriptors.

## Not decomposed

Stories wait for P0 to P3 to land and for the `UNMAPPED` relation to be settled.
