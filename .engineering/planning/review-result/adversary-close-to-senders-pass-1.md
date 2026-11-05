---
format: aep.planning-md/3
id: review-result:adversary-close-to-senders-pass-1
kind: review-result
status: active
title: 'Adversary pass on the bridge close to senders: a feedback-cycle stop refused at the drain fails the run'
relations:
- reviews: story:run-ingress-admitted-send-outlives-the-drain
revision: 1
---
## Pass

This is the adversary pass on the third attempt, Design D, against `e7b8de8`. That commit has the
bridge close each parent edge it holds as known, through `close_to_senders` on `main`'s
`QueueReceiver`. The cases are in `da0afa1`: `adversary1_close_to_senders_cycle.rs` and
`adversary1_close_to_senders_queue.rs`, test files only.

verdict: CONFIRMED
cases: executed 389→392, red 1
origin: introduced 1 / pre-existing 0 / undecided 0

## Cases

- **Red, introduced** (`adversary1_close_to_senders_cycle.rs`): a relay node on a feedback cycle
  through a subgraph node.
  - At the drain, the relay's `stop` waits for room on the full edge into the node. The bridge's
    close refuses it `Closed`.
  - The relay answers that refusal as its error, as in-repo nodes do, and the run ends Failed.
  - On `85bf603` the bridge receives the `stop` and the run completes.
  - Repeated 50 times on each: the unit passed 0 of 50, the base 50 of 50.
  - Only a producer that drains in the same layer as the subgraph node reaches this, which means a
    feedback cycle through it. The public builder accepts and plans such a graph.
- **The consumers of a non-cancel `Closed`.**
  - These treat it as a node error, which fails the drain and then the run:
    - the pipeline nodes: vad, turn policy, commit gate, tool authority and tool executor;
    - the testkit's graph, model, speech-to-text, text-to-speech and cascade nodes;
    - the audio device's acknowledging node;
    - the subgraph egress bridge.
  - These do not: the audio device node, the facade's live session, switch, replay, and the
    local voice cascade example.
  - None of them panics.
- **Green.**
  - A closed queue refuses a send as `Closed` with its own envelope, under each of `pause`, `fail`,
    `drop`, `coalesce` and `drop_stale_epochs`, before the policy's full-queue answer.
  - A send dropped at each of four points around the close pushes nothing.
  - A `recv` future alive at the close cannot be written, because both methods borrow `&mut self`.
  - Every construction of the guard wraps a `QueueSender`.
- **Suites:** `inference-core` 76 passed; `inference-runtime` 246 passed and 1 failed (the cycle
  case); `inference-facade` 69 passed. Clippy and fmt are clean.

## Routing

The finding goes back to the unit as its one correction. The bridge closes only the edges a run's
ingress feeds, and an edge between two nodes of a run keeps the 0.14.0 path. The cycle case must
then be green without an edit, and the acceptance cases of the Design D section must stay green.

```findings
[{"file":"crates/inference-runtime/src/subgraph.rs","line":1124,"category":"acceptance","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"a node on a feedback cycle through a subgraph node whose stop waits for room at the drain is now refused Closed instead of received, and since nearly every in-repo node treats Closed as its error the run ends Failed where 85bf603 completes (adversary1_close_to_senders_cycle.rs, 0/50 vs 50/50)"}]
```
