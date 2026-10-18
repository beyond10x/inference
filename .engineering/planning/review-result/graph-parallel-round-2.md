---
format: aep.planning-md/3
id: review-result:graph-parallel-round-2
kind: review-result
status: active
title: aep:plan-critic-parallel-safety on epic:graph-minimum, round 2
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

**What I read.** 20 artifacts: the epic and the seven stories in the set (each with `aep plan artifact show`), round 1 (`review-result:graph-parallel-round-1`), the contracts siblings `story:workspace-skeleton`, `story:testkit-conformance-target`, `story:inbound-text-spans-and-envelope` and `story:llm-contract`, and the rubric. I ran `aep plan artifact list --format json` for the `depends_on` edges. I did not re-run `waves`, since you said no `scope` is recorded.

Round-1 findings, each now fixed:
- **Declaration needs the runner.** `story:pipeline-declaration` now has `depends_on story:testkit-conformance-target`, and the stale "edge to the epic" sentence is gone.
- **Shared files with the contracts stories.** `story:pipeline-declaration` now depends on `story:inbound-text-spans-and-envelope` and `story:llm-contract`, and its Context names the shared files as the reason.
  - The shared files are `crates/inference-testkit/Cargo.toml`, `crates/inference-testkit/src/`, `crates/inference-core` and `ess/SKIPPED.md`.
  - Its Step 5 now confines the testkit work to `tests/target/graph.rs` and `policy.rs`, pre-created by the testkit story (testkit-conformance-target.md, Steps 1 and 2).
  - The outbound story is covered through the inbound edge.
- **Bounded-edges reads the envelope and `Generation.epoch`.** `story:bounded-edges` now has `depends_on` edges to `story:inbound-text-spans-and-envelope` and `story:outbound-text-generations-and-segments`. It also depends on `story:pipeline-declaration` for `Edge` and `OverloadPolicy`.

The set is a single chain, `story:pipeline-declaration` → `story:pipeline-validation` → `story:capability-planning` → `story:run-lifecycle` → `story:node-instance-lifecycle` → `story:text-pipeline-end-to-end`. The one exception is `story:bounded-edges`, which depends only on `story:pipeline-declaration` and the two contracts stories. That is the stated parallel pair: it runs beside `story:pipeline-validation` and `story:capability-planning`, and its own body says so (bounded-edges.md, Context).

On that pair:
- **graph.yaml:** `story:bounded-edges` edits nothing in `ess/domains/graph.yaml`, which the other two edit.
- **`ess/SKIPPED.md`:** `story:bounded-edges` removes no ids from it.
- **Crate and manifest:** `story:bounded-edges` creates `inference-runtime`, and the root manifest globs `crates/*` (workspace-skeleton step 1).
- **Pipeline crate:** it only reads the `inference-pipeline` types that `story:pipeline-declaration` lands, so there is no shared file.

Surfaces: 7 of 7 items placed, all **cited**, 0 inferred, 0 unplaceable.

Acceptance reads traced to a producer: 12 inside the set, and all 12 are covered by `depends_on`. Nine are direct edges and three run transitively down the chain.
- The three transitive reads are the adapter store for planning, and the run commands and the builder reads for the end-to-end story.

Four reads trace to a producer outside the set, and all four are covered:
- The runner is covered by a direct edge to `story:testkit-conformance-target`.
- The envelope and `Generation.epoch` are covered by direct edges to the two contracts stories.
- The mock LLM and the `EmitTextDelta` and `CommitSegment` answers reach `story:text-pipeline-end-to-end` through `story:bounded-edges` and `story:pipeline-declaration`.

**What I could not establish.**
- **Edges to epics.** I could not tell whether `waves` treats a `depends_on` edge to an epic as gating the stories under it. No finding rests on `story:text-pipeline-end-to-end` → `epic:contracts-and-testkit`, because its real reads now arrive through story edges.
- **`Cargo.lock`.** I could not tell whether a `Cargo.lock` is committed. If it is, `story:bounded-edges` (new crate) and `story:capability-planning` (proptest dev-dependency) both regenerate it. That is a mechanical merge and I made no claim.
- **Compile-level coupling.** `story:bounded-edges` builds against `inference-pipeline` while `story:pipeline-validation` and `story:capability-planning` add to it. Nothing in the bodies suggests changes to the types `story:bounded-edges` reads, so I counted it as ordinary dependency use.
- **Lib-file text in `story:pipeline-declaration`.** Its Context says the contracts stories add one `mod` line to `crates/inference-testkit/src/lib.rs`. `story:testkit-conformance-target` step 1 says no later story adds a line there. Every pair is ordered by an edge, so it is not a collision; it is a stale sentence that could be corrected.
- **Out of my lane.** Whether `story:pipeline-validation` and `story:capability-planning` should be one story belongs to the design critic. Sibling stories outside the set (tools, audio, tts) were not assessed.

```findings
[]
```
