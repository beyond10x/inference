---
format: aep.planning-md/3
id: review-result:review-030-runtime
kind: review-result
status: active
title: Read-only review of inference-runtime since 0.3.0 (haiku)
relations:
- reviews: story:bounded-edges
prose_only: read-only general-purpose reviewer requested by the operator; findings are prose, each traced to a line
revision: 1
---
defects found: 4 (none is a panic, lost wakeup or deadlock on the main paths; I traced the mutex discipline and found those sound)

Scope: `git diff 979ca4d..main` for `inference-runtime`, `inference-core/src/endpoint.rs` and `lib.rs`. I read the full `queue.rs` on main, the endpoint traits, the pipeline `Edge`/`OverloadPolicy` types, design §12.3 and §19, and the graph.yaml overload enum. I did not run cargo.

1. **Receiver drop silently discards queued envelopes and does not count them.** `crates/inference-runtime/src/queue.rs:413-422`, `QueueReceiver::drop`, calls `state.items.clear()` and never increments `state.discarded`. The module doc at `queue.rs:24` says "Every discarded or replaced envelope adds one to `QueueReceiver::discarded`", and the `discarded()` doc at `queue.rs:348` says the same.
   - Trigger: `Pause` edge, capacity 2. The producer sends A and B, both get `Ok`. The consumer drops the receiver. `QueueSender::discarded()` stays 0 while A and B are gone.
   - Confidence: verified by reading the code path. The doc mismatch is certain.
   - Design relevance: §12.3 says committed text must be "deliver or return an explicit failure; never silently discard". The design does not say what happens to queued items when the consumer dies, so whether this is a defect against the design is a question for the owner.

2. **Coalesce drops the newest envelope when no queued envelope shares its stream.** `queue.rs:213-225`. If no queued item has the arriving `stream_id`, the arrival is discarded (`queue.rs:223`). This is documented at `queue.rs:14-15`, so it is a documented choice.
   - Trigger: `Coalesce`, capacity 2, queue `[s1:a, s1:b]`. Arrival `s2:c` is discarded, so the consumer only sees the older `s1` items.
   - Design relevance: §12.3 says "Coalesce/supersede **old** hypotheses". Here the policy keeps old envelopes and drops the new one.
   - Confidence: suspected. The code matches its own doc, and the mismatch is with design intent.

3. **A stale pending `DropStaleEpochs` send stays blocked until the consumer pops.** `queue.rs:186-204`. A purge wakes senders only when `freed` is set, which requires that some item of the same producer was actually removed (`queue.rs:186`). A newer-epoch arrival from producer P with no P items queued leaves `freed` false. P's pending lower-epoch sends, registered at `queue.rs:204`, are not woken. They wait for a `poll_recv` pop (`queue.rs:233-235`) before they are discarded at `queue.rs:172-174`.
   - Trigger: `DropStaleEpochs` edge, capacity 1, filled by producer Q. Producer P has a pending epoch-1 send. P then sends epoch 2. The epoch-1 send stays pending until the consumer takes Q's item, even though it is already stale.
   - Design relevance: §12.3 "drop stale epochs on interruption". The interruption does not discard the stale send until the queue drains.
   - Confidence: verified by reading the wake logic. The test at `tests/adversary1_wakeups.rs:186` covers only the case where P had items queued, so it does not catch this.

4. **Doc claims the runtime builds queues per run, and no code does.** `crates/inference-runtime/src/lib.rs:2-5` says the crate "turns a declared pipeline into running nodes and routable streams". `crates/inference-core/src/endpoint.rs:9-10` says "the runtime builds one queue per edge per run". A `git grep` on main finds no `EdgeQueue::new` call outside `queue.rs` and the tests.
   - Confidence: verified by grep. Severity low, doc-only.

Also noted, not reported as defects:
- Dropping a pending send future discards its envelope silently. This is inherent to async cancellation.
- `QueueReceiver::drop` runs payload `Drop` under the mutex, and the wake of pending senders comes after it. A payload whose `Drop` panics would skip the wake and leave pending senders without `Closed`. This needs a panicking payload, so I rate it low and suspected only.
