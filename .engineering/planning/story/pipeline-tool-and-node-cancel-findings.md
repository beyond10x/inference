---
format: aep.planning-md/3
id: story:pipeline-tool-and-node-cancel-findings
kind: story
status: active
title: Pipeline tool authority, executor and stream nodes settle every answer, honour cancel and stay bounded
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
- serves: vision:inference
scope:
- confidence: inferred
  path: crates/inference-pipeline/src/definition.rs
- confidence: inferred
  path: crates/inference-pipeline/src/lib.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/commit_gate.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/tool_authority.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/tool_executor.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/turn_policy.rs
- confidence: inferred
  path: crates/inference-pipeline/src/nodes/vad.rs
revision: 10
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T00:41:03Z", actor: "human:timo", revision: 9}
- {from: "proposed", to: "active", at: "2026-11-06T00:41:03Z", actor: "human:timo", revision: 10}
---
## Context

From review-result:full-codebase-2026-11-06 (findings W5, N16 to N23), at release 0.16.0, all in `crates/inference-pipeline`:

- W5 `src/nodes/tool_authority.rs:264-272`: `signal()` pops one queued answer; when that call was withdrawn by `interruption()` it drops it without examining the next, and `answer()` refuses a re-push, so a confirmed call is stranded and `drain` can hang.
- N16 `src/nodes/tool_executor.rs:290`, `:302-333`: the read-ahead queue is unbounded while a call runs.
- N17 `tool_executor.rs:442`, `:457-458`, `:161-167`: a call delivered just before `cancel()` is moved to Started and its executor polled once before the cancel check.
- N18 `src/nodes/vad.rs:314-322`, `turn_policy.rs:221-229`, `commit_gate.rs:432-435`: sends are not cancel-aware, unlike the tool nodes' `send_unless`.
- N19 `src/definition.rs:430`: a feedback loop's `max_iterations` is not capped as a capacity is.
- N20 `src/nodes/commit_gate.rs:13-15`: the doc names a `wrong-state` outcome that `stream.yaml` does not declare (`immutable`).
- N21 `tool_authority.rs:48-51`: the doc says end of input passes on when the input edge closes; the code passes it on stop.
- N22 `src/lib.rs:1187-1190`: the `FeedbackLoop` doc counts rounds, the authority counts proposals.
- N23 `commit_gate.rs:146-150`, `:121-133`, `:203`, `:258`: sets and record vectors are never pruned; `record.dropped.contains` is O(n).

## Planned scope

- `signal()` loops over queued answers until it finds a live one or the queue is empty.
- Bound the read-ahead queue to the input edge's capacity.
- Check cancel before moving a call to Started or polling its executor.
- VAD, turn policy and commit gate send through the cancel-aware send the tool nodes use.
- Cap `max_iterations` in `from_json` as the builder caps a capacity.
- Correct the three docs (N20, N21) and make the feedback-loop count and its doc agree (N22, the doc follows the code unless the ESS says otherwise).
- Prune commit-gate state per finished turn; a set for dropped ids.

## Acceptance

- Two held calls, the first withdrawn after its answer queued: the second is authorized on the next poll and `drain` ends.
- An executor running a call while upstream sends more than the edge capacity applies backpressure.
- A cancel racing a delivered call leaves no Started call without `ToolCallStarted`.
- A cancel while a VAD, turn-policy or commit-gate send waits on a full edge ends the wait.
- `"max_iterations": 9223372036854775808` is refused or capped as the builder does.
- A long run's commit-gate state stays bounded by open turns.
