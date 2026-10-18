---
format: aep.planning-md/3
id: review-result:graph-scope-round-2
kind: review-result
status: active
title: aep:plan-critic-scope on epic:graph-minimum, round 2
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

**What you read**
- 8 artifacts, each shown whole with `aep plan artifact show`: `epic:graph-minimum` (revision 3) and its 7 stories. I also showed `review-result:graph-scope-round-1`.
- `aep plan artifact graph` found no other claimant of the epic's promises.
- I read `ess/domains/policy.yaml`, which declares `ProfileId`, `ProfileKind`, `Egress`, `DeploymentProfile`, the command `DeclareProfile` and the view `Profiles`.
- I read `ess/domains/graph.yaml` for `NodeRole`, where `tool` exists, which check 9 needs.
- I checked `.engineering/drafts/suite.json` mechanically. It holds 153 scenarios, 85 of them `inference.graph` and 1 `inference.policy`.
  - The 66 ids listed under `story:run-lifecycle` equal the suite's Run, `DescribeNodeInstance` and run-command scenarios exactly.
  - The other 19 `inference.graph` scenarios split 7, 5 and 7 across declaration, validation and planning, so none is missing or claimed twice.

Promises extracted from the epic: 20. Traced to an item: 20. The 20 are the Outcome (1), the Scope bullets (13, counting the `inference.policy` types and the testkit `DeclareProfile`/`Profiles` answer as separate promises) and the Exit bullets (6). I recounted because the epic grew by two promises in revision 3.

**Round-1 finding.** It no longer holds. Both parts of the claim are fixed:
- **Epic.** Its Scope now has the `inference-core` bullet that owns the `inference.policy` types, the testkit answering `inference.policy.DeclareProfile` and the view `inference.policy.Profiles`. The Context paragraph gives the reason, and the Exit names `inference.policy.DeclareProfile/outcome/declared`. The edit is in `epic:graph-minimum` revision 3.
- **Story.** The `story:pipeline-declaration` Context paragraph, previously line 29 as in round 1, now says "The epic Scope therefore owns the `inference.policy` types and the scenario `inference.policy.DeclareProfile/outcome/declared`". Its Acceptance claims 8 ids, 7 graph and 1 policy, which matches the suite.

**Check 4.** The round-1 note that it was claimed only by outcome is also fixed. The `story:capability-planning` Context now opens "it is §10.2 check 4 (mandatory model, tool and duplex capabilities are satisfied)".

**Coverage by promise**

| Promise | Claimed by |
|---|---|
| Declaration: Pipeline, Node, Port, Edge, Binding, typed builder | `story:pipeline-declaration` |
| §10.2 checks 1, 2, 3, 5, 6, 9; §23.3 tests 1 and 2 | `story:pipeline-validation` |
| Check 4, §8.3, A-13, property test | `story:capability-planning` |
| Run lifecycle, cancel and drain commands | `story:run-lifecycle` |
| Bounded edges, §12.3 policies, §23.3 test 4 | `story:bounded-edges` |
| NodeInstance lifecycle and its ESS extension | `story:node-instance-lifecycle` |
| A-02, §23.3 test 7, the closing `ess/SKIPPED.md` check | `story:text-pipeline-end-to-end` |

- Narrowings are stated in the stories themselves, so they are not silent. These are check 3 covering text commitment only, check 5 refusing every cycle, and no telemetry channel or jitter management in bounded edges.
- No item reaches into the epic's Out of scope. Real adapters, tools, sessions, manifests and subgraphs appear only as exclusions.
- Re-running the closing suite in `story:text-pipeline-end-to-end` step 5 is marked "the re-run, not a new claim", so it is not a double claim.

**What you could not establish**
- `ess/SKIPPED.md` does not exist yet. `story:testkit-conformance-target` creates it, so I could not check that the ids each story removes match its contents.
- Whether the end-of-input control event that `story:text-pipeline-end-to-end` step 2 adds to `inference-core` (only if the contracts epic has not) can collide with a sibling story. It is traceable to A-02, so it is not reach. Collision is for the parallel-safety critic, and the sibling stories are not mine to judge.
- Out of my lane, not counted in the verdict:
  - The Context text of `story:node-instance-lifecycle` says the 66 Run scenarios keep passing after re-synthesis because the runner emits no `NodeInstance*` events. Whether that holds is a design and acceptance question.
  - The `story:pipeline-declaration` Context says it "is the head of the graph chain", while the relations show `depends_on` edges to three stories of `epic:contracts-and-testkit` as well. That is a prose and dependency question.

```findings
[]
```
