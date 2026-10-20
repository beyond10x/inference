---
format: aep.planning-md/3
id: decision-blocker:usage-subgraph-rollup
kind: decision-blocker
status: cleared
title: Nobody has decided whether a run usage total includes the usage of its subgraph child runs
summary: Parent run to child run usage roll-up, and whether child pipelines forward usage, are undecided; totals for pipelines with subgraph nodes wait on it.
relations:
- blocks: epic:usage-and-cost
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-10-20T11:06:14Z", actor: "human:timo", revision: 3}
---
## Question

Does the usage total of a run include the usage of the child runs its subgraph nodes create?

## Why it is open

A subgraph node runs its child pipeline as a child run, one per subgraph node instance per parent
run (`ess/domains/graph.yaml`, `inference.graph.Run`, field `parent` and relation
`parent_instance`). A usage record names the one run it was made in (`inference.usage.UsageRecord`,
relation `run`, as `story:usage-domain` declares it). Nothing says whether the parent run total
rolls up the child runs, nor whether a child pipeline must expose the `usage` ports of its model
nodes so that the records reach the session at all. The design is silent (`docs/design/inference.md`
544, 1150), and its open question 7 (line 1231) on durable versus in-memory event delivery does not
answer it.

## Options

- Roll up: a run total includes its child runs; a subgraph node forwards the child usage on its own
  `usage` port, and each record keeps the child run id.
- Separate: a run total counts only records naming that run; the session total counts every record
  it receives, child ones included once forwarded.
- Not forwarded: usage inside a subgraph stays on the child run and the caller reads it there.

## What it stops

Usage totals for pipelines with subgraph nodes. `story:usage-session-totals` covers pipelines without
them and says so. No other drafted story depends on the answer.

## Decision

Taken on 2026-10-20 by the coordinating session under the owner's "decide, don't ask" rule for
choices inside an approved design: **Roll up.** A run total includes its child runs; a subgraph
node forwards the child usage on its own `usage` port, and each record keeps the child run id, so
the child total stays derivable. Reason: usage must never be lost (the epic's outcome), and a
subgraph is one node to its parent (A-14), so its cost belongs to the parent run. Reversible: the
records carry their own run ids either way. `story:usage-session-totals` extends to pipelines with
subgraph nodes, or a follow-up story does.
