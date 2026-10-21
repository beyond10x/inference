---
format: aep.planning-md/3
id: story:usage-unconnected-port-counted
kind: story
status: draft
title: Usage that reaches no edge is counted or marks the run total partial
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
revision: 1
---
## Context

Wave 26 (`story:subgraph-unconnected-usage-port`) made an unconnected output port of a subgraph node
read and drop the child's egress, matched to a model node whose port has no edge: that node gets no
endpoint and never sends its usage record. In both cases the run's usage total leaves the usage out
and nothing records that it did, so a session total can read complete while usage was spent.

## Planned scope

- Usage that reaches no edge, from a model node with no `usage` edge or a subgraph node whose `usage`
  port has none, is still counted in the run's usage total, or the total is marked partial with the
  node named; it is never left out silently.
- `ess/domains/usage.yaml` and `graph.yaml` state which of the two holds.

## Acceptance

A pipeline whose model node, and one whose subgraph node, has no edge on its `usage` port reports a
run usage total that either includes that node's usage or is marked partial naming the node.

- `task check` passes.
