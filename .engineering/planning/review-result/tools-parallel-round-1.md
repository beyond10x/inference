---
format: aep.planning-md/3
id: review-result:tools-parallel-round-1
kind: review-result
status: active
title: aep:plan-critic-parallel-safety on epic:tools-and-turns, round 1
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

cascade-read-only-tool — its LLM-node change (start generation at `TurnCompleted` by default) lands on the LLM node that tool-call-interruption also edits (inferred, no path named), `depends_on` runs from both to tool-feedback-loop but not between them, and neither body says so. Remedies, not chosen here: an ordering edge recording the node as its reason, or splitting the node's start trigger from its reinjection guard. — .engineering/planning/story/cascade-read-only-tool.md:33, .engineering/planning/story/tool-call-interruption.md:32
endpoint-controller — its body names no crate or path for the VAD node, the turn-policy node or the testkit audio fixture (only `inference-audio` as an input), and it has no `depends_on`, so it would run in the first wave beside tool-call-lifecycle. Its surface is unplaced against the node crate the authority, executor and LLM nodes land in and against `inference-testkit` (inferred, not safe); the body needs to name its crate and files. — .engineering/planning/story/endpoint-controller.md:24-40, .engineering/planning/story/tool-authority-and-executor-nodes.md:34-37
tool-call-interruption — both this and story:tool-safe-retries edit the executor node, the executor contract and the mock executor in `inference-testkit` (cited, both bodies), and `depends_on` runs from each only to the authority or loop story, so one wave holds both. The two bodies do not mention each other. The mock capability this story's test needs (a Started call held until released) is added by no story's scope: the authority story lists scripted result, failure and deadline, and safe-retries adds failure injection. Remedies, not chosen here: an ordering edge that records the shared files as its reason, or splitting the mock executor and executor node so the two stories no longer share them; and the body must say it adds the hold-until-release behaviour. — .engineering/planning/story/tool-call-interruption.md:29-31, .engineering/planning/story/tool-call-interruption.md:37-39, .engineering/planning/story/tool-safe-retries.md:25-27, .engineering/planning/story/tool-safe-retries.md:36-37, .engineering/planning/story/tool-authority-and-executor-nodes.md:41-43

**What you read:** 7 stories, the epic, and 14 sibling stories (13 under `epic:contracts-and-testkit` and `epic:graph-minimum`, plus `story:pipeline-validation`'s feedback paragraph). Commands: `aep plan artifact show` for the epic and each story, `aep plan artifact list --format json` for the `depends_on` edges, `aep plan artifact waves --kind story --status draft` (0 waves, 21 unassessed, as expected), and `grep` over the sibling bodies for crates and spec files.

**Surfaces established**
- Cited: 4 (tool-call-lifecycle, tool-authority-and-executor-nodes, tool-feedback-loop, tool-safe-retries).
- Inferred: 2 (tool-call-interruption and cascade-read-only-tool name roles and neighbour stories, not paths).
- Unplaced: 1 (endpoint-controller).

**Acceptance reads**
- 11 reads trace to a producer in the set; all 11 are ordered by a `depends_on` edge, 6 directly and 5 through the chain lifecycle, authority, feedback-loop.
- 1 read has no producer in the set: the hold-until-release mock in tool-call-interruption (finding above).

**What I could not establish**
- Whether the authority node and the executor node share a file, so I cannot say whether tool-feedback-loop (bound enforcement in the authority) collides with tool-safe-retries (executor node).
- Where the VAD node, the turn-policy node and the LLM node live. `inference-pipeline` (authority story) and `inference-runtime` (node-instance-lifecycle) both claim the node code, and `inference-pipeline` depends only on `inference-core`, a design question for the design critic.
- Whether tool-safe-retries adds an attempt count to the `ToolCallFailed` type in `ess/domains/tools.yaml`. Its edge to the lifecycle story orders that, so it is not a concurrency finding.
- Collisions with sibling stories: none found. The sibling `graph.yaml`, testkit and `tools.yaml` edits precede this set through `epic:tools-and-turns` depends_on `epic:local-voice-cascade` depends_on `epic:graph-minimum`. Those edges are epic-level, and I do not know whether `waves` honours them for stories.
- `epic:local-voice-cascade` has no stories yet, so cascade-read-only-tool's reuse of its A-04 test is unchecked.

```findings
- file: .engineering/planning/story/cascade-read-only-tool.md
  line: 33
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the LLM-node change (start generation at TurnCompleted by default) lands on the LLM node that story:tool-call-interruption also edits, an inferred surface since neither body names a path; both depend on story:tool-feedback-loop with no edge between them and neither body says so, so name the node and order them or split the start trigger from the reinjection guard"
- file: .engineering/planning/story/endpoint-controller.md
  line: 24
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the body names no crate or path for the VAD node, the turn-policy node or the testkit audio fixture and has no depends_on, so the story is unplaced and would run in the first wave beside story:tool-call-lifecycle; its collision with the node crate and inference-testkit is inferred, and the body needs to name where it lands"
- file: .engineering/planning/story/tool-call-interruption.md
  line: 29
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "both this and story:tool-safe-retries edit the executor node, the executor contract and the mock executor in inference-testkit (cited, both bodies) with no depends_on between them, so one wave holds both and neither body mentions the other; its test also needs a hold-until-release mock behaviour no story scope adds (the authority story lists scripted result, failure and deadline; safe-retries adds failure injection), so order the two or split the shared files, and state that this story adds the behaviour"
```
