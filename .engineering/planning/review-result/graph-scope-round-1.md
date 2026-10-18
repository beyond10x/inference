---
format: aep.planning-md/3
id: review-result:graph-scope-round-1
kind: review-result
status: active
title: aep:plan-critic-scope on epic:graph-minimum, round 1
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

story:pipeline-declaration — claims the `inference.policy` types (`ProfileId`, `ProfileKind`, `Egress`, `DeploymentProfile`) in `inference-core` and the scenario `inference.policy.DeclareProfile/outcome/declared`, and nothing in the epic asks for either (the epic's Scope names only `inference-pipeline` and `inference-runtime`, and its Exit names only `inference.graph` scenarios), so either the story drops the claim or the epic gains the sentence that owns it — .engineering/planning/story/pipeline-declaration.md:29 (compare .engineering/planning/epic/graph-minimum.md:34 and :46)

**What you read**
- 8 artifacts: the epic and its 7 stories, via `aep plan artifact show <id>`.
- Also `aep plan artifact graph`, `relations` and `epic:contracts-and-testkit`. I also read `docs/design/inference.md` §10.2, §12.2, §12.3, §23.3 and §23.4, `ess/domains/graph.yaml` (Port and NodeRole), `.engineering/drafts/suite.json`, `story:workspace-skeleton`, and the `inference.policy` lines of `story:testkit-conformance-target` and the ESS artifact.
- Promises extracted from the epic: 18. Traced to an item: 18.
  - The 18 are: Outcome (1), Scope bullets (11), Exit bullets (5), counted by clause.
  - 18 of 18 have an item. The only mismatch is the reach finding above.
- The suite holds 153 scenarios, 85 of them `inference.graph`.
  - Declaration claims 7, validation 5, planning 7 and run-lifecycle 66. That is 85, with none missing and none claimed twice.
  - I listed the 19 non-Run scenarios against the suite by id.
- Promises of the Exit and Scope, each with the story that claims it:
  - §10.2 checks 1, 2, 3, 5, 6 and 9: `story:pipeline-validation`.
  - Check 4, §8.3, A-13 and the property test: `story:capability-planning`.
  - Run and NodeInstance lifecycles: `story:run-lifecycle` and `story:node-instance-lifecycle`.
  - §12.3 policies and test 4: `story:bounded-edges`.
  - A-02, cancel, drain and test 7: `story:text-pipeline-end-to-end`.
  - Tests 1 and 2: `story:pipeline-validation`.
- Narrowings I found are stated in the story itself and are not silent, so they are not findings:
  - Check 3 covers text commitment only. `inference.graph.Port` has no encoding or timebase field (`ess/domains/graph.yaml:179-200`).
  - Check 5 refuses every cycle, because no feedback noun exists.
  - `story:bounded-edges` leaves out the telemetry channel and jitter management, both listed under its Out of Scope.

**What I could not establish**
- Whether `inference.policy.DeclareProfile` belongs to this epic.
  - `story:testkit-conformance-target` (a sibling, not mine to judge) already names `epic:graph-minimum` as the owner of that scenario. So the fix may be a sentence in the epic. The story's own text says "no epic owned that scenario".
- Check 4 is claimed only by outcome. `story:capability-planning` never names §10.2 check 4. Only `story:pipeline-validation` points to it, so one citation there would make the claim traceable. I counted this as covered, not as a finding.
- Out of my lane, not counted in the verdict:
  - `story:pipeline-declaration` Context line 29 says the `depends_on` edge points at the epic. The graph shows `depends_on story:workspace-skeleton`, so the prose is stale.
  - After `story:node-instance-lifecycle` re-synthesizes the suite, the run-command scenarios still assert `expect_no_event` for `NodeInstance*` events while the run commands now drive instances. I did not check whether the 66 scenarios of `story:run-lifecycle` survive that. This is a design and acceptance question.

```findings
- file: .engineering/planning/story/pipeline-declaration.md
  line: 29
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "story:pipeline-declaration claims the inference.policy types in inference-core and the scenario inference.policy.DeclareProfile/outcome/declared, and nothing in epic:graph-minimum asks for either (Scope names only inference-pipeline and inference-runtime, Exit names only inference.graph scenarios), so either the story drops the claim or the epic gains the sentence that owns it"
```
