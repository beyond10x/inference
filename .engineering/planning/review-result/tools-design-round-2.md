---
format: aep.planning-md/3
id: review-result:tools-design-round-2
kind: review-result
status: active
title: aep:plan-critic-design on epic:tools-and-turns, round 2
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

story:cascade-read-only-tool — its acceptance reads the mock synthesiser and the playback-acknowledging sink double that story:tts-contract creates, and no depends_on edge runs from it to story:tts-contract; the earlier edges (feedback-loop, endpoint-controller, interruption) do not reach it, so add depends_on story:tts-contract — .engineering/planning/story/cascade-read-only-tool.md:56, .engineering/planning/story/tts-contract.md:31

**Round-1 findings (`review-result:tools-design-round-1`)**

| Round-1 finding | Status | Line that changed |
|---|---|---|
| tool-feedback-loop, bound ownership and the edge to `story:pipeline-validation` | Fixed, whole | `tool-feedback-loop.md:12` (edge); `:25` and `:53` state that this story declares the bound and amends check 5. `pipeline-validation.md` check 5 agrees. |
| tool-feedback-loop, edge to `story:text-pipeline-end-to-end` | Fixed, whole | `tool-feedback-loop.md:13` |
| tool-authority-and-executor-nodes, edge to the graph-minimum story | Fixed, whole | `tool-authority-and-executor-nodes.md:12` (to text-pipeline-end-to-end, which brings the node-instance trait) |
| tool-call-lifecycle, edge to `story:testkit-conformance-target` | Fixed, whole | `tool-call-lifecycle.md:11` |
| endpoint-controller, three edges | Fixed, whole | `endpoint-controller.md:11-13`, plus a fourth edge at `:14` |

The `tts-contract` finding is new, not a repeat. The cascade story was revised to name that story's mock synthesiser and sink double, and round 1 did not raise it.

**What I read:** 8 artifacts in full (the epic and the 7 stories). I also read `story:tts-contract`, `story:text-pipeline-end-to-end`, `story:node-instance-lifecycle`, `story:pipeline-validation` and `review-result:tools-design-round-1`. I ran `aep plan artifact relations`, `graph`, `validate` (55 artifacts, valid), `list --format json` and `waves`.

**Edges walked:** 56 `depends_on` edges, and 141 non-review edges in all, outside the set as well.
- I found no cycle.
- The longest chain in the set is five stories, testkit-conformance-target, tool-call-lifecycle, tool-authority-and-executor-nodes, tool-feedback-loop, tool-call-interruption, then cascade-read-only-tool.
- The two ordering edges, interruption to safe-retries and cascade to interruption, each carry a written reason (shared executor node, mock executor and LLM node files) and the trade-off against splitting those files. Under the rubric that is not a chain finding.

**Acceptance reads:** I traced about 20 reads to a producer. 19 have an edge, direct or by a short path through the story-level edges. 1 has none, the `tts-contract` read above.

**What I could not establish:**
- `aep plan artifact waves` has no scope on any story, so it printed no waves. I could not see whether epic-level `depends_on` edges gate stories.
- `story:cascade-read-only-tool` reads the commit-gate node and the A-04 network-deny harness of `epic:local-voice-cascade`. That epic has no stories, so its only record is the epic edge, which the body names at `cascade-read-only-tool.md:34`. If epic edges do not gate stories, a story-level `depends_on epic:local-voice-cascade` would be the record. `story:text-pipeline-end-to-end` already targets an epic that way.
- If epic edges do gate stories, `epic:tools-and-turns` depends on `epic:local-voice-cascade`, which is blocked by an open decision-blocker. That would hold the six mock-only stories behind it, though they need only mocks.
- Out of my lane, so they did not set the verdict:
  - `story:endpoint-controller` and `story:tool-authority-and-executor-nodes` both add new files under `crates/inference-pipeline/src/nodes/`, with no edge between them. Which story creates `nodes/mod.rs` is a parallel-safety question.
  - `story:endpoint-controller` (Where it lands) still hedges "if that trait's crate forces the authority and executor nodes elsewhere", although `story:node-instance-lifecycle` step 2 now fixes the crate.
  - `story:tool-call-interruption` relies on the executor node taking two authorized calls in order, one at a time. No story's scope adds that serial behaviour.

```findings
- file: .engineering/planning/story/cascade-read-only-tool.md
  line: 56
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its acceptance reads the mock synthesiser and the playback-acknowledging sink double that story:tts-contract creates, and no depends_on edge runs from it to story:tts-contract; the earlier edges (feedback-loop, endpoint-controller, interruption) do not reach it, so add depends_on story:tts-contract"
```
