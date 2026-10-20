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
of each exposure, and drives every node instance as the run moves. `RunController::drive` drains
the run once every source has sent end of input, completes it once every instance has drained,
and takes cancel, drain or fail requests from the caller. A node that returns an error fails the
run with a reason naming it.

## Backpressure

An edge queue holds at most the edge's capacity; its overload policy decides what a full queue does
with the next event. Every sender refuses user content whose sequence does not increase on its
stream, and the run records the refusal (`SequenceNotIncreasing`).

## Cancellation and drain

- **Drain** admits no new input and lets in-flight events complete, upstream nodes first.
- **Cancel** stops the run; a subgraph's child run keeps its own cancellation boundary.

## Replay and telemetry <span class="st st-rel">0.6.0</span>

- A `Recorder` sits on a run's event path and records every envelope in the merge order it
  assigns, and the control decisions among them.
- A `Replay` feeds a recording's inputs into a new run of the same pipeline and reports the first
  control decision that diverged.
- A `TelemetrySink` takes telemetry on a bounded, lossy channel that drops and counts what a slow
  consumer has no room for.

## Egress watch <span class="st st-rel">0.6.0</span>

On Linux, a run whose profile denies egress fails once a socket of the process connects anywhere
but loopback. See [Offline and privacy](../offline-and-privacy.md).
