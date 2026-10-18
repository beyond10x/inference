---
format: aep.planning-md/3
id: review-result:graph-acceptance-round-2
kind: review-result
status: active
title: aep:plan-critic-acceptance on epic:graph-minimum, round 2
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
approve

All three round-1 findings are fixed, and I found nothing new in the acceptance lane.

Round-1 findings and what changed:
- `story:node-instance-lifecycle` (blocker, round 1 finding 1): fixed. The acceptance now carries the before and after as facts. It requires `ess specify validate --path ess` to report `valid`. It requires `graph.yaml` to declare the six commands, `inference.graph.NodeInstanceStateConflict` and the `state` field on `inference.graph.NodeInstances`. It states that none of these exist today. The changed line is `.engineering/planning/story/node-instance-lifecycle.md:56`. I checked `ess/domains/graph.yaml` and none of the six commands, `PrepareNodeInstance`, `FailNodeInstance` or `NodeInstanceStateConflict` is declared there now, so the before state is true.
- `story:bounded-edges` (warning, round 1 finding 2): fixed. The vague phrase is gone from the acceptance at line 55. Checks 3 to 5 at lines 63 to 65 name the delivered set and the discard count for each policy:
  - `drop`: items 1 to `c` are read and the count is `c`.
  - `coalesce`: items 1 to `c - 1` followed by item `2c` are read, and the count is `c`, then `c + 1`. I traced the replace-newest-same-stream rule by hand and it gives exactly these numbers.
  - `drop_stale_epochs`: only the epoch 2 item is read and the count is 2, then 3.
- `story:text-pipeline-end-to-end` (warning, round 1 finding 3): fixed. Context line 29 fixes the drain point after input 3 is emitted and before input 4, names `s3` as the in-flight segment, and gives the sink contents `s1, s2, s3`. Check 3 at line 57 asserts those contents, that input 4 was never admitted, and the Completed state.

Nothing else changed the verdict. The acceptances of `story:pipeline-declaration`, `story:pipeline-validation`, `story:capability-planning` and `story:run-lifecycle` still name observable outcomes. Each has a before state ("today `unsupported`, each named in `ess/SKIPPED.md`") and a named change in that file. The epic's Exit list is observable too.

**What I read:** 8 artifacts, all 8 given (the epic and 7 stories), each in full with `aep plan artifact show <id>`. I also read `review-result:graph-acceptance-round-1`, `aep plan artifact kinds` and `aep plan artifact lifecycle story`. I extracted every `inference.graph.*` and `inference.policy.*` scenario id the artifacts name and checked it against `.engineering/drafts/suite.json` (153 scenarios, 85 under `inference.graph`). No named id is missing, and the four stories that list ids claim all 86 `inference.graph` and `inference.policy` ids between them.

**What I could not establish:**
- The scenario ids that `story:node-instance-lifecycle` will add after re-synthesis do not exist yet, so I could not check them.
- `ess/SKIPPED.md` does not exist yet, because `story:testkit-conformance-target` creates it. The "today `unsupported`" claims therefore cannot be checked against a file.
- Out of my lane, and they did not set the verdict:
  - The round-1 observation still stands that `story:node-instance-lifecycle` step 1 omits a `stop` transition from Failed, which sits awkwardly with "`FailRun` stops each instance". Line 34 now says "cancel then stop each instance that is not already terminal", which may resolve it.
  - Line 26 now explains why `fail` and `cancel` leave Described, so the earlier prepare-from-Described gap looks closed. This is a design-lane check, not mine.
  - `story:bounded-edges` check 5 queues two items, which needs `c >= 2`. The story does not fix `c`. This is a test-design detail, not an acceptance defect.

```findings
[]
```
