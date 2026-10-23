---
title: Session facade
sidebar_label: Session
sidebar_position: 6
description: One application-facing interface over any pipeline, cascade or native model, with history across runs, switching by configuration, and usage totals.
lede: The application reaches a run only through the pipeline's exposed ingresses and egresses, so it sees one event vocabulary and no topology.
source: crates/inference-facade (session.rs, live.rs, describe.rs, usage.rs, cost.rs), ess/domains/session.yaml
---

# Session facade

`inference-facade` is the optional, application-facing layer. No other crate needs it, and a
pipeline runs without one.

A session is an interaction lifetime above graph runs. It is opened over one planned pipeline and
holds its current run, zero or one. The application reaches the run only through the pipeline's
exposed ingresses and egresses, so it sees no topology.

| Operation | Does |
|---|---|
| open | opens a session over a planned pipeline |
| send | sends input to the session |
| receive | yields one event vocabulary (`SessionEvent`), whatever pipeline answers |
| commit input | commits the user's input |
| interrupt | interrupts the active generation |
| describe | reports what runs now: nodes, bindings, effective capabilities, downgrades, exposures |
| close | closes by draining, or by cancelling |

- **History** keeps committed user turns, assistant segments and tool results across runs.
- **Switch**: a ready session moves to another planned pipeline, draining its run first and
  keeping its history and vocabulary. The same session switches between the text-only pipeline
  and the voice cascade by configuration.

What a session reads of the graph it does not hold, it reads through the `session::Graph` trait
its host implements. The facade runs in-process, over no executor in particular.

## Usage totals and cost

Since 0.8.0, a live session delivers each usage record of its run as
`SessionEvent::UsageRecorded`, and `LiveSession::usage` returns a `SessionUsage`: a total per run
the session held, in the order it attached them, and one over the session, across switches. A
record counts when the session reads it, whether or not it is delivered, so an interrupt or a
cancel does not lose it. A run's total includes the usage of its subgraphs whose `usage` port has
an edge.

With the facade's `cost` feature, `LiveSession::price_with` prices every usage record the session
reads with a price document you supply, and `SessionUsage::cost` holds a cost record per usage
record and their total. How the totals and prices behave is in [Usage and cost](usage.md).
