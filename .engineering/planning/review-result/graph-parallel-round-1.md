---
format: aep.planning-md/3
id: review-result:graph-parallel-round-1
kind: review-result
status: active
title: aep:plan-critic-parallel-safety on epic:graph-minimum, round 1
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

story:pipeline-declaration — its acceptance and step 5 need the conformance runner that story:testkit-conformance-target creates, but its only story edge is `depends_on story:workspace-skeleton`, so one wave can hold both. The stale sentence naming an edge "to the epic" should go with the fix, and the missing edge to the producer should be added, or the two stories merged — .engineering/planning/story/pipeline-declaration.md:57, .engineering/planning/story/pipeline-declaration.md:11, .engineering/planning/story/testkit-conformance-target.md:27

story:pipeline-declaration — it can run beside story:outbound-text-generations-and-segments, story:inbound-text-spans-and-envelope and story:llm-contract on three shared files, and no body admits it. The files are `crates/inference-testkit/tests/conform.rs` (the target every one extends; cited), `ess/SKIPPED.md` (cited for the contracts stories, inferred for the graph stories) and `crates/inference-core` (the policy types here, `Generation` and the envelope there; cited). The remedies are an ordering edge that records the shared files as its reason, or splitting the surface. The graph chain is ordered internally, so one edge at its head or a stated split covers the rest — .engineering/planning/story/pipeline-declaration.md:39, .engineering/planning/story/testkit-conformance-target.md:28, .engineering/planning/story/outbound-text-generations-and-segments.md:30, .engineering/planning/story/inbound-text-spans-and-envelope.md:31

story:bounded-edges — its tests build items on the §6.2 event envelope that story:inbound-text-spans-and-envelope creates in `inference-core`. They also use `Generation.epoch`, which story:outbound-text-generations-and-segments creates. Its only edge is `depends_on story:run-lifecycle`, so the reader is scheduled beside its producers. The missing edges should be added, or the stories merged — .engineering/planning/story/bounded-edges.md:22, .engineering/planning/story/bounded-edges.md:52, .engineering/planning/story/inbound-text-spans-and-envelope.md:31, .engineering/planning/story/outbound-text-generations-and-segments.md:28

**What I read.** 17 artifacts, via `aep plan artifact list --format json`, `aep plan artifact waves --kind story --status draft` and `aep plan artifact show`.
- The epic and all seven stories of the set.
- The contracts siblings: workspace-skeleton, testkit-conformance-target, outbound-text-generations-and-segments, inbound-text-spans-and-envelope and llm-contract.
- The tools siblings: epic:tools-and-turns, tool-call-lifecycle, endpoint-controller and tool-authority-and-executor-nodes.
- A grep of the other tools and contracts stories for `graph.yaml`, `inference-runtime`, `inference-pipeline`, `SKIPPED`, `conform.rs`, `Cargo.toml` and `Taskfile`.

Surface counts for the set:
- 7 of 7 items placed, all **cited**. Four are single-crate (`inference-pipeline` for the first three; `inference-runtime` for bounded-edges and text-pipeline-end-to-end), three are mixed. 0 inferred, 0 unplaceable. `ess/SKIPPED.md` is **inferred** for the graph stories (they omit it), but the testkit story's rule that every later story removes its ids is cited.
- The set is one linear `depends_on` chain: declaration, validation, capability-planning, run-lifecycle, bounded-edges, node-instance-lifecycle, text-pipeline-end-to-end. So nothing in the set runs at once, and the shared `graph.yaml`, `inference-pipeline`, `inference-runtime` and `conform.rs` are ordered. `aep plan artifact waves` computed nothing, since no scope is recorded.
- Acceptance reads traced to a producer inside the set: 8, all 8 recorded by a `depends_on` edge.
- Acceptance reads traced to a producer outside the set: 4, namely the runner for declaration, `DeclareAdapter` held by the runner for capability-planning, the envelope and `Generation.epoch` for bounded-edges. 0 of the 4 are recorded by a story edge. The text-pipeline-end-to-end read of the mock LLM is covered only by an edge to the epic.

**What I could not establish.**
- Whether `aep plan artifact waves` treats a `depends_on` edge to an epic (text-pipeline-end-to-end to `epic:contracts-and-testkit`, and `epic:tools-and-turns` to `epic:local-voice-cascade` to `epic:graph-minimum`) as gating the stories under it. I raised no finding against either edge.
- Whether the workspace `Cargo.toml` lists its members or globs `crates/*`, since no code exists. I made no collision claim on it.
- Tools siblings. story:tool-authority-and-executor-nodes lands nodes in `inference-pipeline` and reads the graph API of pipeline-declaration, but has only `depends_on story:tool-call-lifecycle`. It is ordered after this set only by epic edges. If those edges do not gate stories, it needs an edge to the graph stories. The finding would sit on that story, which is outside the set, so I did not file it.
- story:tool-feedback-loop may declare the feedback noun in `ess/domains/graph.yaml`. story:pipeline-validation names that hand-off, so I treated it as named.
- Out of my lane: whether story:pipeline-validation and story:capability-planning should be one story (design critic).

```findings
- file: .engineering/planning/story/pipeline-declaration.md
  line: 57
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance reads the testkit conformance runner that story:testkit-conformance-target creates (testkit-conformance-target.md:27), and the only story edge is depends_on story:workspace-skeleton (line 11), so the reader and its producer can share a wave; add the depends_on edge to story:testkit-conformance-target (or merge) and drop the stale sentence at line 31 saying the edge points at the epic"
- file: .engineering/planning/story/pipeline-declaration.md
  line: 39
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "this story can run beside story:outbound-text-generations-and-segments, story:inbound-text-spans-and-envelope and story:llm-contract on crates/inference-testkit/tests/conform.rs (cited), ess/SKIPPED.md (cited for the siblings, inferred for the graph stories, which omit it) and crates/inference-core (cited), and no body names the overlap; record it as an ordering edge giving the shared files as its reason, or split the surface"
- file: .engineering/planning/story/bounded-edges.md
  line: 52
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance runs the queue over the event envelope that story:inbound-text-spans-and-envelope creates in inference-core (inbound-text-spans-and-envelope.md:31) and over Generation.epoch that story:outbound-text-generations-and-segments creates (outbound-text-generations-and-segments.md:28), and the only edge is depends_on story:run-lifecycle; add the two depends_on edges (or merge)"
```
