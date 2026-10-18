---
format: aep.planning-md/3
id: review-result:graph-design-round-1
kind: review-result
status: active
title: aep:plan-critic-design on epic:graph-minimum, round 1
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

story:pipeline-declaration — its acceptance reads the `ConformanceTarget` runner and in-memory stores that story:testkit-conformance-target creates, and no `depends_on` edge runs to that story; Context line 31 still says the edge points at the epic, when the graph shows only `story:workspace-skeleton`. Add `depends_on story:testkit-conformance-target` and correct the sentence. Later stories reach it through the chain — `.engineering/planning/story/pipeline-declaration.md:57`, `.engineering/planning/story/testkit-conformance-target.md:16`, `aep plan artifact graph`

story:bounded-edges — its `drop_stale_epochs` and `coalesce` tests read the event envelope and `Generation.epoch`, but no edge runs to either producer: story:inbound-text-spans-and-envelope declares the envelope (`stream_id`, `generation_id`, no `epoch` field) and story:outbound-text-generations-and-segments declares `Generation.epoch`. The body never says how the queue gets from `generation_id` to an epoch. Add `depends_on` to both stories, or to `epic:contracts-and-testkit` as story:text-pipeline-end-to-end does, and state where the epoch is read — `.engineering/planning/story/bounded-edges.md:22`, `.engineering/planning/story/bounded-edges.md:32`, `.engineering/planning/story/inbound-text-spans-and-envelope.md:18`

story:bounded-edges — its only edge, `depends_on story:run-lifecycle` (line 10), has no reason written beside it. Its acceptance (line 50) is queue-only tests that read nothing run-lifecycle produces, so the set is a strict seven-item queue, and the run-lifecycle edge is the only step in it not forced by a stated read. The shared surface is the `inference-runtime` crate that run-lifecycle creates. Two ways to close it: keep the edge and write "crate created by run-lifecycle" beside it, or drop it and have bounded-edges create the crate module itself, so it runs beside validation, planning and run-lifecycle after the declaration story. Do not just delete the edge — `.engineering/planning/story/bounded-edges.md:10`, `aep plan artifact graph`

## What I read
- 7 stories plus `epic:graph-minimum`, whole bodies, with `aep plan artifact show`.
- Also read: `story:workspace-skeleton`, `story:testkit-conformance-target`, and the bodies of `story:inbound-text-spans-and-envelope`, `story:outbound-text-generations-and-segments` and `story:llm-contract` as needed.
- Commands run: `aep plan artifact relations`, `aep plan artifact graph`, `aep plan artifact validate` (reported `valid`), and `grep` over `ess/domains/stream.yaml`.
- I walked all 120 edges in the store graph, outside the set included. No cycle. The chain inside the set is declaration → validation → planning → run → bounded-edges → node-instance, with node-instance also depending on run, and end-to-end last.
- I traced 9 acceptance reads to a producer: 6 are recorded by an edge and 3 are not. The 3 unrecorded reads are the runner (finding 1) and the envelope and epoch (finding 2).
- Recorded reads: validation → declaration, planning → validation, run → planning, node-instance → run and bounded-edges, end-to-end → node-instance and bounded-edges.

## What I could not establish
- Whether the epic-level `depends_on epic:contracts-and-testkit` is meant to cover story-level reads. It makes every graph story wait on the whole sibling epic (tts, audio and the rest), which is coarser than any story needs. I left it alone because it is a dependency on a sibling epic.
- Out of my lane (acceptance, parallel safety): six graph stories each must remove their ids from `ess/SKIPPED.md`, and `story:testkit-conformance-target` asserts `unsupported == SKIPPED`. Only the graph stories' bodies do not say so (grep for `SKIPPED` over them returned nothing). That is a shared file and a test that goes red.
- Out of my lane (acceptance, parallel safety): the 66 scenarios in `story:run-lifecycle` and the 85 in the epic exit are counts synthesized before steps 1 of validation, planning and node-instance-lifecycle change `graph.yaml`. I did not check them.
- Out of my lane (scope): the end-of-input control event that `story:text-pipeline-end-to-end` takes "from `inference-core`" is not named as a deliverable of `story:inbound-text-spans-and-envelope`.

```findings
- file: .engineering/planning/story/pipeline-declaration.md
  line: 57
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its acceptance reads the conformance runner and in-memory stores that story:testkit-conformance-target creates, and no depends_on edge runs to that story (Context line 31 still says the edge points at the epic, while the graph shows only story:workspace-skeleton); add depends_on story:testkit-conformance-target and correct the sentence"
- file: .engineering/planning/story/bounded-edges.md
  line: 32
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its drop_stale_epochs and coalesce tests read the event envelope (story:inbound-text-spans-and-envelope) and Generation.epoch (story:outbound-text-generations-and-segments), no edge runs to either, and the body does not say how the queue resolves generation_id to an epoch; add the depends_on edges and state where the epoch is read"
- file: .engineering/planning/story/bounded-edges.md
  line: 10
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "depends_on story:run-lifecycle has no reason beside it and the acceptance (queue-only tests) reads nothing run-lifecycle produces, so it makes the seven stories a strict queue; either write 'crate created by run-lifecycle' beside the edge or drop it and have bounded-edges create the inference-runtime crate module itself"
```
