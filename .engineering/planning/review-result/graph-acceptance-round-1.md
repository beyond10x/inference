---
format: aep.planning-md/3
id: review-result:graph-acceptance-round-1
kind: review-result
status: active
title: aep:plan-critic-acceptance on epic:graph-minimum, round 1
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

story:node-instance-lifecycle — the acceptance passes vacuously if Step 1 is never done: "every scenario the re-synthesized suite derives for `inference.graph.NodeInstance` and its six new commands" is the empty set when `ess/domains/graph.yaml` still declares only Described. The acceptance should require that `ess specify validate --path ess` is valid and that graph.yaml declares the six commands, with `NodeInstanceStateConflict` and the extended `NodeInstances` view, as the before/after fact — .engineering/planning/story/node-instance-lifecycle.md:57

story:bounded-edges — "under `drop` and `coalesce` the delivered items are the ones each policy keeps" defines the expected result by the policy itself, so it names no checkable output. It should state the delivered set, for example the first `capacity` items and the discard count for `drop`, and which items survive a coalesce burst. The Step 4 test ("delivers the newest") leaves open whether the earlier queued items on that `stream_id` are kept — .engineering/planning/story/bounded-edges.md:52

story:text-pipeline-end-to-end — "the drained run delivers its in-flight segments" does not say which segments count as in flight, that is, how many segments the source has emitted when `DrainRun` is issued. Two runs of the test could therefore assert different sink contents. The acceptance should name the expected sink contents, or the point at which the drain is issued — .engineering/planning/story/text-pipeline-end-to-end.md:54

**What I read:** 8 artifacts (the epic and the 7 stories), each through `aep plan artifact show <id>` in full. I also read `docs/design/inference.md` at lines 1122-1150 and `ess/domains/graph.yaml` for `NodeRole`. I extracted every `inference.graph.*` and `inference.policy.*` scenario id the artifacts name and checked it against `.engineering/drafts/suite.json` (153 scenarios, 85 under `inference.graph`). No named id is missing. The four stories that name scenarios (declaration, validation, planning, run-lifecycle) together claim all 85 `inference.graph` ids. The 66-item list in `story:run-lifecycle` counts exactly 66.

**What I could not establish:**
- `aep plan artifact kinds` and `aep plan artifact lifecycle <kind>` were not run. The lifecycle names did not affect any acceptance judgment.
- The scenario ids that `story:node-instance-lifecycle` will add after re-synthesis cannot be checked yet, because they do not exist.
- The other acceptances were judged as conjunctions of independent, checkable items. These cover the scenario lists plus Rust tests in `story:pipeline-declaration`, `story:pipeline-validation`, `story:capability-planning` and `story:run-lifecycle`. I did not treat a conjunction as a "more than one statement" defect, because each is done only when every item passes.
- Out of my lane, which did not set the verdict:
  - `story:node-instance-lifecycle` Step 1 allows `fail` only from Prepared, Started and Draining. Its prose says an error in prepare moves the instance to Failed, and prepare runs from Described. It also says `FailRun` stops each instance, but `stop` is not allowed from Failed.
  - `story:text-pipeline-end-to-end` Step 3 has the loop advance a run to Completed on its own, while the three-run description issues `DrainRun` and `CompleteRun` explicitly.

```findings
- file: .engineering/planning/story/node-instance-lifecycle.md
  line: 57
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance passes vacuously if Step 1 is never done, because the scenarios derived for the six new commands are the empty set while graph.yaml declares only Described; it should require ess specify validate to be valid and graph.yaml to declare the six commands, NodeInstanceStateConflict and the extended NodeInstances view"
- file: .engineering/planning/story/bounded-edges.md
  line: 52
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "'under drop and coalesce the delivered items are the ones each policy keeps' defines the expected result by the policy itself; it should name the delivered set and the discard count for drop and the surviving items of a coalesce burst"
- file: .engineering/planning/story/text-pipeline-end-to-end.md
  line: 54
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "'the drained run delivers its in-flight segments' does not say which segments are in flight when DrainRun is issued, so the expected sink contents are not fixed; it should name them or the point of the drain"
```
