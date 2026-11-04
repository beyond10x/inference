---
format: aep.planning-md/3
id: review-result:adversary-subgraph-nested-drain-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the nested drain fix: a pending receive is taken for an empty edge (pre-existing)'
relations:
- reviews: story:subgraph-nested-drain-residual-loss
revision: 1
---
## Pass

Adversary pass 1 on story:subgraph-nested-drain-residual-loss, against `8b47266` on
`impl/subgraph-nested-drain-residual-loss`; the cases are `7498ad9`,
`crates/inference-runtime/tests/adversary1_drain_window_edges.rs` (test file only), built on the
sleep-free window of `subgraph_drain_window.rs`.

verdict: red
cases: executed 0→3, red 1
origin: introduced 0 / pre-existing 1 / undecided 1

## Cases

- `text_on_the_first_of_two_edges_crosses_while_the_bridge_is_held_on_the_second`: ok.
- `text_on_the_second_of_two_edges_crosses_while_the_bridge_is_held_on_the_first`: ok. The two
  ran 200 times each, sequential, no rebuild: 400/400.
- `text_on_an_edge_whose_receive_is_ready_only_on_a_later_poll_crosses`: FAILED. A receive that
  answers `Pending` once, wakes itself and then hands over the envelope it holds: `Ended {
  drained: Ok(()), child: Some(Completed), texts: [], left_on_the_edge: 2 }`.
- Command: `cargo test -p inference-runtime --test adversary1_drain_window_edges`, 2 passed, 1
  failed; fmt and clippy clean.

Read from the code and not broken: a lost wakeup when the drain reads false (both the bridge
state and every edge register the waker), a `stop` racing the drain, a grandparent forwarding
bridge, cancel against drain.

## Routing

The red case is pre-existing by reading, not by a run on the base: before the fix the bridge
also took a pending receive for an empty edge. The runtime's own receivers pend only when empty,
so no edge inside the crate reaches it. It is filed as
`story:drain-treats-a-pending-receive-as-an-empty-edge` (draft, from-finding), and the case stays
on the unit branch at `7498ad9`, not merged, because it is red by design.

The undecided finding (a run ingress admits a send that can still wait for room after the drain)
is filed as `story:run-ingress-admitted-send-outlives-the-drain` (draft, from-finding, measure
first). The unit merges at `8b47266` without change for either.

The second finding was returned with severity `info`, a value the brief offered by mistake; it
is recorded as `note`, the store's nearest severity; its verdict `UNDECIDED` is not a store verdict,
so the entry carries none and its origin `undecided` says the same. Nothing else is changed.

```findings
[{"file":"crates/inference-runtime/src/subgraph.rs","line":904,"category":"data-loss","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"The ingress bridge ends for the drain when every edge's receive answered Pending, so it treats Pending as an empty edge. The InputEndpoint::recv contract ('waiting while the edge is empty') does not promise that Pending means empty. A receive that pends once (wake_by_ref) while holding an envelope makes the bridge send stop. The text stays on the edge and is dropped with it, the drain answers Ok and the child completes: silent loss. The runtime's QueueReceiver and SinkReceiver pend only when empty, so in-crate edges are safe; a SubgraphNode driven with another InputEndpoint is not. Red case: tests/adversary1_drain_window_edges.rs text_on_an_edge_whose_receive_is_ready_only_on_a_later_poll_crosses. Fix options: drain only after a non-blocking emptiness check, or state in the InputEndpoint contract that Pending means empty."},
{"file":"crates/inference-runtime/src/drive.rs","line":437,"category":"race","severity":"note","origin":"undecided","message":"The premise 'the parent drains the node only once everything it admitted is on the node's edges' does not hold at a run ingress. GuardedSender checks admits_input on its first poll, then awaits inner.send, which can wait for room under the pause policy. A send admitted before the drain can therefore still be in flight when the subgraph node's bridge finds the edge empty and ends. Its send then answers Closed, or its envelope is discarded with the queue. Code reading only, not reproduced."}]
```
