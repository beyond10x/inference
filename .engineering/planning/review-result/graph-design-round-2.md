---
format: aep.planning-md/3
id: review-result:graph-design-round-2
kind: review-result
status: active
title: aep:plan-critic-design on epic:graph-minimum, round 2
relations:
- reviews: epic:graph-minimum
- reviews: story:pipeline-declaration
- reviews: story:pipeline-validation
- reviews: story:capability-planning
- reviews: story:run-lifecycle
- reviews: story:bounded-edges
- reviews: story:node-instance-lifecycle
- reviews: story:text-pipeline-end-to-end
revision: 1
---
needs-revision

story:node-instance-lifecycle — step 2 puts the node trait in `inference-pipeline` with `start` taking "the input receivers and output senders" of the edges, but those are the sender and receiver sides of the queue that story:bounded-edges step 3 defines in `inference-runtime`, and `inference-pipeline` depends only on `core` and port types, so the trait cannot name them without a pipeline-to-runtime dependency (a crate cycle); the body must say where the endpoint types live (for example the sender/receiver endpoint types, or traits for them, in `inference-pipeline` or `inference-core`, with bounded-edges step 3 adjusted to implement them) — `.engineering/planning/story/node-instance-lifecycle.md:44`, `.engineering/planning/story/bounded-edges.md:44`, `docs/design/inference.md:241-242`

## Round-1 findings

| Round-1 finding | Status | Evidence |
|---|---|---|
| story:pipeline-declaration, missing edge to testkit-conformance-target | Fixed, nothing of it holds | The graph now shows `story:pipeline-declaration -> story:testkit-conformance-target [depends_on]`. Context line 34 states the edge, and line 36 gives the shared files as the reason for the inbound and llm-contract edges. |
| story:bounded-edges, reads of the envelope and `Generation.epoch` with no edge | Fixed, nothing of it holds | The graph shows `story:bounded-edges -> story:inbound-text-spans-and-envelope` and `-> story:outbound-text-generations-and-segments`. Context line 24 and Ambiguities line 77 now state the lookup from `GenerationId` to `epoch`. |
| story:bounded-edges, unexplained `depends_on story:run-lifecycle` | Fixed, nothing of it holds | The edge is gone and bounded-edges now creates `inference-runtime` (Context line 24, step 2). The reason for `run-lifecycle -> bounded-edges` is written at `run-lifecycle.md:23`, and `bounded-edges.md:26` says why there is no edge back. |

The new finding comes from moving the node trait into `inference-pipeline`. The queue and its endpoint types stay in the runtime.

## What I read

- 8 artifacts: the epic and the 7 stories, whole bodies, with `aep plan artifact show`.
- Also read: `review-result:graph-design-round-1`.
- Commands run: `aep plan artifact relations`, `aep plan artifact graph` (237 edges, 56 of them `depends_on`; I read all of them, including edges outside the set), and `aep plan artifact validate` (reported `valid`).
- Also read: `docs/design/inference.md:238-244` for the crate dependency table.
- No cycle. The graph chain is declaration → (validation → planning) and bounded-edges → run-lifecycle → node-instance → end-to-end. Validation, planning and bounded-edges are no longer a strict queue.
- No contracts story depends back on a graph story.
- I traced about 15 acceptance reads to producers in or beside the set. All are recorded by an edge, direct or transitive. The one gap is the node-trait endpoint types, reported above.

## What I could not establish

- Whether `story:inbound-text-spans-and-envelope` and `story:llm-contract` need to precede `story:pipeline-declaration` only for shared files. The bodies say so, so I treated those edges as a trade-off and not a chain. The alternative is splitting `crates/inference-testkit/src/lib.rs`.
- Out of my lane (acceptance): the scenario counts (66 in run-lifecycle, 85 in the epic exit) are fixed to the 2026-10-18 synthesis. Several stories change `graph.yaml` afterwards, and I did not recheck them.
- Out of my lane (parallel safety): `ess/SKIPPED.md` and `crates/inference-testkit/tests/conform.rs` are shared across stories. The bodies now say which story removes which ids.

```findings
- file: .engineering/planning/story/node-instance-lifecycle.md
  line: 44
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "step 2 puts the node trait in inference-pipeline with start taking the input receivers and output senders of the edges, but those are the sender and receiver sides of the queue story:bounded-edges step 3 defines in inference-runtime, and inference-pipeline depends only on core and port types, so the trait cannot name them without a pipeline-to-runtime dependency (a crate cycle); the body must say where the endpoint types live (for example the sender/receiver endpoint types, or traits for them, in inference-pipeline or inference-core, with bounded-edges step 3 adjusted to implement them)"
```
