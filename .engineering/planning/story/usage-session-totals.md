---
format: aep.planning-md/3
id: story:usage-session-totals
kind: story
status: draft
title: Run and session usage totals in the facade, recorded by the replay recorder and never dropped with telemetry
summary: SessionEvent::UsageRecorded, per-run and per-session totals with unknowns kept, usage in recordings and not compared on replay.
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
- depends_on: story:usage-llm
- depends_on: story:usage-stt
- depends_on: story:usage-tts
revision: 1
---
## Context

Once the STT, LLM and TTS nodes send usage records on their `usage` ports, the session facade is
where an application reads them: `SessionEvent` is its one event vocabulary
(`crates/inference-facade/src/events.rs`), filled from what the run sends on its egresses
(`src/live.rs:909-948`). The runtime recorder already takes every envelope a node emits
(`crates/inference-runtime/src/record.rs:1-30`) and counts as a control decision only the kinds
`DecisionKind::of` names (`record.rs:112-134`), so a usage envelope is recorded and not compared.
Telemetry is the best-effort channel that drops when its consumer lags (`src/telemetry.rs:1-11`);
the epic requires that usage never depends on it.

## Planned scope

- `inference-core` `usage`: `UsageTotal`, per quantity kind the sum of the known values, the number
  of `unknown` quantities and the number of records, with `complete` true only when no quantity of
  that kind was unknown; the same shape `llm-cost` gives a cost total (known subtotal, unknown
  lines). Unknown is never added as 0.
- `inference-facade`: `SessionEvent::UsageRecorded`, delivered like any payload event; the live
  session keeps a total per run it held and one for the session, read through a `usage()` view;
  the session total covers every run the session held across `switch_pipeline`. The module
  documentation names the scope: pipelines whose nodes are not subgraph nodes, see Not covered.
- `inference-testkit`: the voice cascade (`src/cascade.rs`) and the text pipeline expose the
  `usage` ports of their model nodes as egresses, and `SessionPayload` converts the usage payload.
- `inference-runtime`: `Recording::usage()` lists the usage envelopes of a recording in merge
  order; a test pins that `DecisionKind::of("inference.usage.UsageRecorded")` is none, so replay
  never compares usage.

## Not covered

Whether the total of a run includes the usage of the child runs of its subgraph nodes is undecided:
`decision-blocker:usage-subgraph-rollup`. This story counts usage for pipelines without subgraph
nodes only and its tests use none.

## Domain relations

- UsageRecord to Run and to Session, each zero or one: `ess/domains/usage.yaml`,
  `inference.usage.UsageRecord`, relations `run` and `session`, as `story:usage-domain` declares them.
- A session total spans the runs the session held: inferable, inferred from
  `ess/domains/session.yaml` `inference.session.Session` (summary: a session may span several graph
  runs; field `run_id` comment: over its lifetime a session references several runs, one at a time).
  The total is a value the session keeps, not an entity; no `relations:` entry declares it.

## Acceptance

A voice-cascade session on the testkit mocks answering one spoken turn delivers one
`SessionEvent::UsageRecorded` per model call (one STT, one LLM turn, one TTS) and a session usage
total equal, kind by kind, to the sum of those records, with every unknown quantity counted as
unknown.

- With a telemetry channel of capacity 1 whose receiver is never read, the same session still
  delivers all three records and the telemetry channel reports drops.
- The recording of that run lists the three usage envelopes, and a replay whose mock reports other
  counts reports no divergence.
- A session that switches from the text pipeline to the cascade keeps one total per run and a
  session total over both.
- `task check` passes.
