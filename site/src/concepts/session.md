# Session facade <span class="st st-rel">0.5.0</span>

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

Run and session usage totals in the facade are <span class="st st-plan">planned</span>
([Usage and cost](../usage-and-cost.md)).
