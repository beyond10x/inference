---
format: aep.planning-md/3
id: review-result:adversary-pipeline-tool-cancel-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the pipeline tool and node cancel fixes
relations:
- reviews: story:pipeline-tool-and-node-cancel-findings
revision: 1
---
unit: 0.18.0 RF-pipeline story:pipeline-tool-and-node-cancel-findings, unit head 458cbe6; the adversary's case is commit 96e3c00 (one test file)
verdict: NEEDS-CHANGE
cases: executed 30→31, red 1
origin: introduced 1 / pre-existing 1 / undecided 0

**Case** (`crates/inference-pipeline/tests/adversary_rf1_tool_executor.rs`): a call is delivered, its `ToolCallStarted` send waits on an edge that stays full, then the cancel comes; the authority's record still holds the call in Started with nothing sent. Red at `:359`.

**Suite**: lib 21/0, adversary3 cancel 1/0 and interruption 1/0, review_fix_cancel_sends 3/0, review_fix_definition_iterations 1/0, review_fix_tool_nodes 3/0; clippy clean.

**Attacked, not broken**: dead answers skipped in a loop (W5); the three nodes send through the cancel-aware send (N18); `carried` shared with the builder (N19); the doc fixes (N20-N22); public additions only.

Cargo 0.4 min, wall about 30 min, peak `target/` 251M.

```findings
[
{"file":"crates/inference-pipeline/src/nodes/tool_executor.rs","line":655,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"A cancel while ToolCallStarted waits on a full edge leaves the call Started in the authority's record with nothing sent, against the acceptance; fixable by restoring the delivered call under the lock."},
{"file":"crates/inference-pipeline/src/nodes/commit_gate.rs","line":228,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"pre-existing","message":"stale is pruned only when a dropped segment's commit arrives, a cancelled generation emits SegmentDiscarded instead, and ended grows per generation, so commit-gate state is not bounded by open turns."},
{"file":"crates/inference-pipeline/src/nodes/commit_gate.rs","line":124,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A revision of a span committed two or more turns back is now counted in held_back, whose field doc still says revisions of spans not yet committed."},
{"file":"crates/inference-pipeline/src/nodes/tool_executor.rs","line":591,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"At the default read-ahead of 64 an interruption behind 64 queued calls of the running call's generation does not fire that call's signal until it ends; documented, nothing found sends more than 64 calls in one generation."}
]
```
