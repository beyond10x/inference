---
format: aep.planning-md/3
id: review-result:tools-design-round-1
kind: review-result
status: active
title: aep:plan-critic-design on epic:tools-and-turns, round 1
relations:
- reviews: epic:tools-and-turns
- reviews: story:tool-call-lifecycle
- reviews: story:endpoint-controller
- reviews: story:tool-authority-and-executor-nodes
- reviews: story:tool-feedback-loop
- reviews: story:tool-safe-retries
- reviews: story:tool-call-interruption
- reviews: story:cascade-read-only-tool
revision: 1
---
needs-revision

tool-feedback-loop — The body leaves ownership of the feedback-bound declaration open ("if the graph-minimum stories declared a feedback bound … otherwise declare it there"). `story:pipeline-validation` already says check 5 refuses every cycle and that this epic declares the feedback noun. The acceptance ("validating with the bound declared and refused without it") reads the `ValidatePipeline` check that `story:pipeline-validation` produces, and no `depends_on` runs to it. The body should state that this story declares the bound in `ess/domains/graph.yaml` and amends check 5 to accept a bounded cycle. It should also add `depends_on story:pipeline-validation`. — .engineering/planning/story/tool-feedback-loop.md:40, .engineering/planning/story/tool-feedback-loop.md:72, .engineering/planning/story/pipeline-validation.md:29

tool-feedback-loop — Its acceptance runs "the text-only pipeline on mocks with the feedback edge" and the scope has the LLM node gain ports. That node and pipeline are produced by `story:text-pipeline-end-to-end` ("the model node wrapping the mock LLM adapter"). The dependency is reached only through the authority story's unrecorded graph dependency, not by a `depends_on` edge. An edge to `story:text-pipeline-end-to-end` would record it. — .engineering/planning/story/tool-feedback-loop.md:29, .engineering/planning/story/text-pipeline-end-to-end.md:24

tool-authority-and-executor-nodes — The scope builds the test pipeline "on the graph API of graph-minimum" (scripted source, nodes, recording sink, bounded edges). It also writes both nodes against the node-instance trait and says the nodes live "with the node implementations graph-minimum established". Those come from `story:node-instance-lifecycle` and `story:text-pipeline-end-to-end` (the mock source and sink). The only edge is `depends_on story:tool-call-lifecycle`, so a wave could place it beside them. It needs `depends_on story:text-pipeline-end-to-end`, which covers the trait, bounded edges and mock source and sink. — .engineering/planning/story/tool-authority-and-executor-nodes.md:44, .engineering/planning/story/node-instance-lifecycle.md:42

tool-call-lifecycle — Its acceptance is `ess verify conform run --path ess --target <testkit>`, and scope item 3 extends the conformance target "answers … `inference.model` and `inference.stream` … answers `inference.tools` too". That target, with its in-memory stores and `ConformanceTarget`, is created by `story:testkit-conformance-target`. This story declares no `depends_on` at all. An edge to `story:testkit-conformance-target` would record it. — .engineering/planning/story/tool-call-lifecycle.md:66, .engineering/planning/story/testkit-conformance-target.md:7

endpoint-controller — It declares no `depends_on` edge, yet its acceptance reads state three siblings produce. First, the `OpenTurn` and `CompleteTurn` scenarios pass only against a testkit target that stores Turns, which `story:inbound-text-spans-and-envelope` adds. Second, the VAD node takes `inference-audio` frames, declared by `story:audio-and-stt-contract`. Third, "the nodes in the loop" need the node-instance trait and runtime from `story:node-instance-lifecycle`. It needs edges to `story:inbound-text-spans-and-envelope`, `story:audio-and-stt-contract` and `story:node-instance-lifecycle`. — .engineering/planning/story/endpoint-controller.md:26, .engineering/planning/story/endpoint-controller.md:50, .engineering/planning/story/inbound-text-spans-and-envelope.md:52

**What I read:** I read 8 artifacts in full: the epic and the 7 stories. I read the bodies of 7 siblings (`story:pipeline-validation`, `story:pipeline-declaration` in part, `story:text-pipeline-end-to-end`, `story:testkit-conformance-target`, `story:node-instance-lifecycle`, `story:inbound-text-spans-and-envelope`, `story:audio-and-stt-contract` in part, `story:llm-contract` in part). I ran `aep plan artifact relations`, `graph` and `validate`, which reported 42 artifacts and valid. I read `ess/domains/tools.yaml` and grepped `ess/domains/graph.yaml`.

**Edges walked:** I walked all edges in the store graph, so outside the set as well.
- No cycle. Within the set the edges are lifecycle → authority → {feedback-loop, safe-retries}, feedback-loop → {interruption, cascade-read-only-tool}, and endpoint-controller → cascade-read-only-tool.
- It is not a serialising chain: the longest path is 4, and safe-retries, interruption and endpoint-controller can run beside their siblings.

**Acceptance reads:** I traced about 12 acceptance reads to a producer. 5 are produced inside the set and all 5 have an edge. 7 are produced by siblings outside the set and none has a story-level edge (the 5 findings above). Only the epic-level `depends_on` chain through `epic:local-voice-cascade` to `epic:graph-minimum` exists.

**What I could not establish:**
- Whether an epic-level `depends_on` is meant to gate its stories in `aep plan artifact waves`. If it is, the sibling findings above weaken to a recording gap. The epic edge to `epic:local-voice-cascade` would also hold the lifecycle, authority and feedback stories, which need only graph-minimum and contracts, behind that whole epic.
- `story:cascade-read-only-tool` reads the local adapters and the A-04 test of `epic:local-voice-cascade`. That epic has no stories yet, so no story edge can be named. The epic edge is the only record.
- Out of my lane, so it did not set the verdict:
  - `story:tool-call-interruption` assumes the mock executor "holds a Started call until released", but no story's scope adds that behaviour (parallel-safety or acceptance).
  - `story:tool-call-lifecycle` does not say it updates the `ess/SKIPPED.md` lines that `story:testkit-conformance-target` writes for the 3 `inference.tools` ids (acceptance or parallel-safety).
  - `story:tool-safe-retries`, `story:tool-call-interruption` and the authority story all extend the mock executor and the authority node (parallel-safety).

```findings
- file: .engineering/planning/story/tool-feedback-loop.md
  line: 40
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the body leaves ownership of the feedback-bound declaration open while story:pipeline-validation refuses every cycle and assigns the noun to this epic, and its acceptance reads the ValidatePipeline check that story:pipeline-validation produces with no depends_on to it; state that this story declares the bound and amends check 5, and add depends_on story:pipeline-validation"
- file: .engineering/planning/story/tool-feedback-loop.md
  line: 29
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its acceptance runs the text-only pipeline with the LLM node that story:text-pipeline-end-to-end produces, reached only through an unrecorded dependency; add depends_on story:text-pipeline-end-to-end"
- file: .engineering/planning/story/tool-authority-and-executor-nodes.md
  line: 44
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its test pipeline and both nodes are built on the graph API, node-instance trait and mock source and sink that story:node-instance-lifecycle and story:text-pipeline-end-to-end produce, and no depends_on records it; add depends_on story:text-pipeline-end-to-end"
- file: .engineering/planning/story/tool-call-lifecycle.md
  line: 66
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its acceptance runs the conformance target that story:testkit-conformance-target creates and no depends_on records it; add depends_on story:testkit-conformance-target"
- file: .engineering/planning/story/endpoint-controller.md
  line: 50
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it declares no depends_on, yet its acceptance reads Turn scenarios answered by the target that story:inbound-text-spans-and-envelope extends, inference-audio frames from story:audio-and-stt-contract and the node trait from story:node-instance-lifecycle; add those three edges"
```
