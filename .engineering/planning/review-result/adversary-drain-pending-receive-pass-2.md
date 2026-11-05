---
format: aep.planning-md/3
id: review-result:adversary-drain-pending-receive-pass-2
kind: review-result
status: active
title: 'Adversary pass 2 on the pending receive at a drain: nothing blocks; one boxed receive per poll on a known edge'
relations:
- reviews: story:drain-treats-a-pending-receive-as-an-empty-edge
revision: 1
---
## Pass

Adversary pass 2 on story:drain-treats-a-pending-receive-as-an-empty-edge, against `90ce893`
(the corrections: `InputEndpoint::known_contents`, `KnownContents`, the two runtime
implementations, the bounded re-polls for `Unknown` endpoints) on
`impl/drain-treats-a-pending-receive-as-an-empty-edge`; the cases are `a1b8ea3`
(`adversary2_drain_window_receives.rs` changed as the coordinator decided, and
`adversary3_drain_known_edges.rs`, new; test files only).

verdict: nothing found that blocks; 1 note CONFIRMED, 1 INFEASIBLE
cases: executed 6→14, red 0
origin: introduced 1 / pre-existing 0 / undecided 1

## Cases

- The worker-woken case's endpoint now answers `known_contents` from what its edge holds, the
  assertion unchanged: green. Beside it, a case pins the documented limit for an endpoint that
  answers `Unknown` (the drain ends, nothing crosses, 2 envelopes stay on the edge), its message
  naming the story: green.
- `adversary3_drain_known_edges`, 5 green: a spinning holder with a knowing endpoint crosses after
  a 300 ms hold; the same with an unknowing endpoint is lost past the bound (pinned); a known held
  edge beside an unknowing self-waking edge crosses and the drain ends; three busy edges keep
  their order with strict turns (longest run from one edge 1); idle known edges take 0 polls in
  200 ms, then 30 texts a millisecond apart cross in order.
- Regression: `adversary2` 6, `adversary3` 5, `drain_known_contents` 4,
  `adversary1_drain_window_edges` 3, `subgraph_drain_window` 1, `subgraph_nested_drain` 2; clippy
  clean; a repeat loop of both adversary binaries, 40 runs, 0 failures.

Not broken: dropping a pending `QueueReceiver` or `SinkReceiver` receive (no state in the future,
an envelope leaves only when the receive completes, the waker is registered under the lock of the
emptiness check); no lost wakeup between a drop and the next poll; a `NotEmpty` answer never waits
forever for a truthful endpoint; a send landing between `recv` and `known_contents`; mixed known
and unknown edges; `stop` and `None`. Only `QueueReceiver` and `SinkReceiver` implement the trait
in the repository and both answer known, so the `Unknown` path is reached only by outside
implementers.

The base's turn order (`swap_remove`) starved the middle of three busy edges; the new code takes
strict turns.

## Routing

The cost note is accepted and named in the method's documentation. The `Unknown` limit is the
documented one, pinned green. No correction.

```findings
[{"file":"crates/inference-runtime/src/subgraph.rs","line":884,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A known edge costs one boxed recv future per poll (busy run: 381 receives for 123 envelopes, against 123 for unknown endpoints; a spinning holder: 797,136 boxes in about 550 ms), with no effect a caller can see beyond CPU and allocation: order, turn-taking, idle polls and every crossing are unchanged."},{"file":"crates/inference-runtime/src/subgraph.rs","line":1005,"category":"acceptance","severity":"note","verdict":"INFEASIBLE","origin":"undecided","message":"An endpoint that answers Unknown and whose receive holds an envelope while pending until another thread wakes it cannot be told apart from an empty edge without narrowing recv; the documented limit, pinned green by the Unknown-endpoint cases in adversary2 and adversary3."}]
```
