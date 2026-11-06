---
format: aep.planning-md/3
id: review-result:adversary-runtime-bounds-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the runtime subgraph and recorder bounds
relations:
- reviews: story:runtime-subgraph-and-recorder-bounds
revision: 1
---
unit: 0.19.0 RF-runtime story:runtime-subgraph-and-recorder-bounds, unit head ee28de0; the adversary's cases are commit 27f1a1f (one test file)
verdict: NEEDS-CHANGE (two red cases, introduced)
cases: executed 0→8, red 2
origin: introduced 5 / pre-existing 0 / undecided 0

**Cases** (`crates/inference-runtime/tests/adversary_rf1_subgraph_and_recorder_bounds.rs`): nested subgraph drop, drop after an abandoned drain, cancel then stop with a late reader on `fail` and on `pause` edges, recorder bound 0, recorder exactly at its bound: green. Red: a drop after an abandoned stop waits 950 ms for the usage send to give way; the replay of a recording its bound cut reports a divergence at the bound that the run did not make.

**Residual** (a stop with no prior cancel or drain waits for the child): not reachable through `RunController`; only a direct `SubgraphNode::stop` reaches it.

Cargo 0.2 min, wall about 20 min.

```findings
[
{"file":"crates/inference-runtime/src/subgraph.rs","line":269,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"gives_way registers no waker while a usage send sits inside its stop grace, so a Drop after an abandoned stop sets the deadline to now but the send gives way only at the old deadline (950 ms); the Drop comment says at once. Case a_drop_after_an_abandoned_stop_gives_the_usage_send_way_at_once."},
{"file":"crates/inference-runtime/src/record.rs","line":261,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"a recording cut by its bound carries no mark of the cut, so Replay::report compares it as complete and reports a divergence at the bound the run did not make. Case the_replay_of_a_recording_its_bound_cut_reports_no_divergence_the_run_did_not_make."},
{"file":"crates/inference-runtime/src/subgraph.rs","line":802,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"when wait_done times out, Drop reads dropped_usage before the child thread ended, so later give-ups of the detached thread are never counted; the doc says it is counted once the thread returned."},
{"file":"crates/inference-runtime/src/subgraph.rs","line":1478,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"with a usage port fanning out to several parent edges, a record taken by one parent and given up on another is counted whole in dropped_usage, overstating the loss (read, not run)."},
{"file":"crates/inference-runtime/src/subgraph.rs","line":761,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"the residual stop without a prior cancel or drain is reachable only by calling SubgraphNode::stop directly, not through RunController (read, not run)."}
]
```
