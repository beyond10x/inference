---
format: aep.planning-md/3
id: review-result:adversary-drain-pending-receive-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the pending receive at a drain: a self-waking empty edge holds the drain; a worker-woken receive still loses its envelope'
relations:
- reviews: story:drain-treats-a-pending-receive-as-an-empty-edge
revision: 1
---
## Pass

Adversary pass 1 on story:drain-treats-a-pending-receive-as-an-empty-edge, against `ddaf2ab` on
`impl/drain-treats-a-pending-receive-as-an-empty-edge`; the cases are `8416c38`,
`crates/inference-runtime/tests/adversary2_drain_window_receives.rs` (test file only). Every drain
in it has a 4 s deadline the test enforces, so no case can hang the suite.

verdict: CONFIRMED
cases: executed 41→46, red 3
origin: introduced 1 / pre-existing 0 / undecided 1

## Cases

- `the_drain_ends_with_an_empty_open_edge_whose_receive_wakes_itself`: RED. `drained: None,
  drain_ran: 4.000057141s, polls_while_draining: 6663095, child: Some(Cancelled)`.
- `the_drain_ends_and_the_text_crosses_beside_a_self_waking_empty_edge`: RED. `drained: None …
  texts: ["1"]`.
- `a_text_a_receive_holds_until_its_worker_wakes_it_crosses_at_the_drain`: RED. `drained:
  Some(Ok(())), polls_while_draining: 1, texts: [], left_on_the_edges: 2`.
- Green: a stop on a self-waking edge lets the drain end; a cancel after a held drain ends the
  node.
- Regression: the ten runtime targets of the unit, 41 passed; fmt and clippy clean.

No endpoint in the repository wakes itself on an empty poll (`QueueReceiver`, `SinkReceiver`,
adapters, examples); `RunController` hands nodes only `QueueReceiver`s. Outside a drain the bridge
already spins at the same rate on such an endpoint; what is new is that the drain cannot end.

## Routing

The introduced finding goes back to the unit as a correction: bound the extra polls per drain
read. The undecided finding cannot be fixed within the wave's binding constraint (no change to
the public `InputEndpoint` trait or the `recv` contract); it goes to the coordinator with the
options, and its red case does not merge while it stays red.

```findings
[{"file":"crates/inference-runtime/src/subgraph.rs","line":947,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"A receive that wakes itself on every poll of an empty, open edge keeps the recv contract yet now holds the subgraph node's drain until a cancel, the bridge spinning at about 1.6M polls/s (adversary2_drain_window_receives.rs:569, :589); reached only through the public SubgraphNode API with a caller-supplied endpoint, no repository endpoint does it; a bounded number of extra polls per drain read would end it."},{"file":"crates/inference-runtime/src/subgraph.rs","line":950,"category":"acceptance","severity":"warning","verdict":"INFEASIBLE","origin":"undecided","message":"A receive that holds an envelope and answers Pending until a worker on another thread wakes it still loses the envelope at the drain (texts [], left_on_the_edges 2, adversary2_drain_window_receives.rs:647), contrary to the story's acceptance, and the doc at :897 reads recv as waiting only while the edge is empty, which the contract does not say; no fix within the binding constraint since the parent's senders are alive at the drain."}]
```
