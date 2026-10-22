---
title: Graphs
sidebar_position: 4
description: Nodes with typed ports joined by bounded edges, declared three ways, validated, then planned against what each adapter claims.
lede: A pipeline definition is not an execution; it is checked twice before anything runs, and a refusal names the node and what is missing.
source: crates/inference-pipeline (lib.rs), ess/domains/graph.yaml, ess/domains/policy.yaml
---

# Graphs

`inference-pipeline` declares, validates and plans pipelines. A pipeline definition is not an
execution; that is the [runtime](runtime.md).

## Parts

| Part | What |
|---|---|
| `Node` | a model, a policy (voice activity, turn policy, tool authority, commit gate), a tool executor, a fan-out, a source or a sink |
| `Port` | typed by direction and modality |
| `Edge` | connects an output port to an input port with a finite capacity and an overload policy |
| `Exposure` | a port exposed to the application as a named ingress or egress |
| `FeedbackLoop` | bounds the cycles a feedback edge closes, such as the tool round trip |
| `Binding` | binds a node to an adapter, by configuration |

Overload policies: `pause`, `cancel`, `drop`, `coalesce`, `supersede`, `fail`,
`drop_stale_epochs`.

## Three ways to declare

- **Commands**: seven declaration commands, each refusing what the specification refuses.
- **Typed builder**: a port handle carries its direction and modality as types, an edge capacity
  is a `NonZeroUsize`, and a handle cannot leave its builder. An unbounded edge, an edge from an
  input port or a dangling identity cannot be written.
- **Declarative JSON** (`PipelineDefinition`): nodes named by the kind of a published
  `NodeDescriptor`, checked against the descriptors before anything is declared.

## Validated, then planned

**Validation** runs the structural checks over a declared pipeline. A cycle is admitted only
through an edge a feedback loop bounds. A failing graph is refused with `IncompatibleGraph`,
naming each finding.

**Planning** takes only a validated pipeline. It computes the pipeline's effective capabilities
from the weakest node on every required path, and refuses:

| Refusal | When |
|---|---|
| `CapabilityMismatch` | a node needs a capability its adapter does not claim |
| `NodeUnbound` | a node has no adapter for what it requires |
| `LocalityDenied` | an adapter runs at a locality the [deployment profile](deployment-profiles.md) does not admit, inside subgraphs too |

The error names the node and what is missing, before anything runs. Every error kind the
specification declares is in [Errors](../reference/errors.md).

## Subgraphs

A planned pipeline can stand as one node of another; its exposures are the ingress and egress
contracts that node's ports are held to. At run time the child keeps its own cancellation and
tracing boundary.
