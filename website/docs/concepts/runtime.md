---
title: Runtime
sidebar_position: 5
description: How a planned pipeline runs - lifecycle, bounded queues, cancellation and drain, subgraphs, replay, telemetry and the egress watch.
lede: inference-runtime turns a planned pipeline into running nodes and routable streams, and no public signature commits you to one asynchronous executor.
source: crates/inference-runtime (lib.rs, controller.rs, run.rs, subgraph.rs, record.rs, replay.rs, telemetry.rs, egress.rs)
---

# Runtime

`inference-runtime` turns a planned pipeline into running nodes and routable streams. No public
signature commits you to one asynchronous executor.

## Lifecycle

```text
Run:            Created ▶ Validating ▶ Preparing ▶ Active ▶ Draining ▶ Completed
                                   (Failed and Cancelled are explicit states)

Node instance:  Described ▶ Prepared ▶ Started ▶ Draining | Cancelling ▶ Stopped
                                   (Failed is explicit)
```

A run is created from the planned form of a pipeline only and moves by command; a command in the
wrong state is refused. A node implementation writes one trait, `NodeImplementation`: describe,
prepare, start with its edge endpoints, drain or cancel, stop.

`RunController` builds one bounded queue per edge and per exposure, hands the caller the outer end
of each exposure, and drives every node instance as the run moves. Its run loop,
`RunController::drive`, drains the run once every source node has sent its end of input, completes
it once every instance has drained, and takes the cancel, drain or failure its caller asks for. A
node that returns an error fails the run with a reason naming it.

## Backpressure

An edge queue holds at most the edge's capacity; its overload policy decides what a full queue does
with the next event. Every sender refuses user content whose sequence does not increase on its
stream, and the run records the refusal (`SequenceNotIncreasing`).

## Subgraphs

A `SubgraphNode` runs its child pipeline as a child run nested under its own instance, bridges its
ports to the child's exposures, and keeps the run boundary as the cancellation and tracing
boundary. Since 0.8.0, a nested drain waits for the ingress bridges to forward
what they hold; an output port of a subgraph node with no edge no longer fails the run; egress
bridges end with the child run, and an edge that refuses what a bridge forwards fails the drain
(`CHANGELOG.md`, 0.8.0).

## Replay and telemetry

- A `Recorder` sits on a run's event path and records every envelope in the merge order it
  assigns, and the control decisions among them.
- A `Replay` feeds a recording's inputs into a new run of the same pipeline and reports the first
  control decision that diverged.
- A `TelemetrySink` takes telemetry on a bounded, lossy channel that drops and counts what a slow
  consumer has no room for. Usage never travels on it: usage records go on the run's egresses
  ([Usage and cost](usage.md)).

## Egress watch

On Linux, a run whose profile denies egress fails once a socket of the process connects anywhere
but loopback. It is a tripwire, not a sandbox; see
[Deployment profiles](deployment-profiles.md) and [Limitations](../status/limitations.md).
