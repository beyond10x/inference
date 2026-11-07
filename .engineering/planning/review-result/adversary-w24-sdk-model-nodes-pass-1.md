---
format: aep.planning-md/3
id: review-result:adversary-w24-sdk-model-nodes-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the model nodes in inference-pipeline
relations:
- reviews: story:sdk-model-nodes
revision: 1
---
unit: W24-sdk-model-nodes story:sdk-model-nodes, unit head ae88d08 (findings cover ae88d08; adversary commit 70f06b3)
verdict: CONFIRMED
cases: executed 3→4, red 0
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: <scratch> (old-*.rs, new-*.rs, case-1.log, suite-1.log, clippy-1.log, clippy-2.log), …/W24-sdk-model-nodes/msg.vCRMRb
needs-coordinator: yes. Decide whether finding 1 goes back to the implementor. The fix is an implementation file and the adversary may not edit one.

**My case is green. The red evidence is a clippy run, not a test.** On the unit head, `clippy -p inference-testkit --features llm,run -- -D warnings` fails because `send_unless` in the testkit is now dead code.

1. `git --no-pager diff --stat ae88d08 HEAD`: one file, `crates/inference-testkit/tests/adversary_w24_host_payload.rs`, 414 insertions. It is a test file only.

2. Case `adversary_w24_host_payload::a_host_drives_the_pipelines_model_node_on_its_own_payload_as_on_the_text_payload`. It is **green now and was green on its first run.** It takes `ModelNode` from `inference_pipeline::nodes` and runs source, model and sink twice through `RunController`: once on `PipelinePayload`, once on a host payload with no usage variant. It asserts on both runs:
   - one stream of the model's own, numbered 1..n with no gap
   - the `stop` control comes last and is not in the run log
   - the deltas are `["s1","s2","s3"]`
   - every envelope names the generation its payload names
   - the two runs agree on kinds, sequences and the model's log entries

   It has a 20 s `recv_timeout` deadline. Command: `cargo-run.sh test -p inference-testkit --features llm,run --test adversary_w24_host_payload` → `1 passed`, `rc=0 secs=35`.

3. Suite (after the case existed): `cargo-run.sh test -p inference-testkit --features llm,run --test llm_model_node --test adversary_w24_host_payload --test adversary1_llm_node_turns`, exit 0:
   - adversary1_llm_node_turns: 2 passed
   - adversary_w24_host_payload: 1 passed
   - llm_model_node: 1 passed

   The before count of 3 is the same targets without my file. Clippy `-p inference-testkit --features llm,run --test adversary_w24_host_payload -- -D warnings` exited 101 with `error: function send_unless is never used --> crates/inference-testkit/src/graph/nodes.rs:151:21`. With `--features llm,run,stt` it exited 0.

4. Judgement findings (all cover ae88d08):
   - **`send_unless` is now dead code (warning, CONFIRMED).** `crates/inference-testkit/src/graph/nodes.rs:151` is dead unless `stt` is on. At the base, `llm/node.rs` imported it under `llm` (`git show ccd9e37:crates/inference-testkit/src/llm/node.rs`), so the move caused this. It is reached by the wave's own per-crate clippy step (`clippy -p <crate> <features> -D warnings`). `task check` uses `--all-features`, so it does not see it. Fix: gate the function with `#[cfg(feature = "stt")]`, or move `fan_out` onto the pipeline's helper.
   - **The testkit's sink double is now public pipeline API under a neutral name (warning, NEEDS-CHANGE).** `crates/inference-pipeline/src/nodes/tts.rs:621` `PlaybackSink` is the testkit's `MockSink`, renamed. A host that calls `TtsNode::playing()` gets playback acknowledged from an in-memory frame count, with no audio played anywhere. The name hides that it is a double.
   - **The pipeline's own unit test cannot fail (note, CONFIRMED).** `crates/inference-pipeline/src/nodes/mod.rs` `the_model_nodes_and_their_log_live_in_the_pipeline` checks only trait bounds, two empties and `new() == default()`. It would pass with every body stubbed. My case is the behavioural replacement.
   - Residue, not raised: a stale doc line at `examples/local-voice-cascade/src/local.rs:33` ("the testkit's `AgentNode`"). The `local` paths (`local.rs:62`, `examples/talk.rs:51-52`) still import `RunLog`/`StreamEvent` through testkit re-exports. Those resolve to the same pipeline types, so I expect them to compile; this is by reading only, I did not build `local`.

5. Could not break:
   - Emitter conversion: `StreamEmitter` is the old `Emitter` byte for byte. `carry` now takes only `P::stream`, which is equivalent because the old `Control`/`Usage` arms could not be reached.
   - `send_unless` argument reorder: same polling order, cancel checked first.
   - `next` vs `recv_unless` in tts: same order and same three outcomes.
   - `Exchange` and `started`: identical to the base.
   - Cancel/supersede `stop` envelopes: `P::from(kind)` on both sides.
   - Normal dependency tree of inference-pipeline: inference-llm/stt/tts pull no mock, testkit or runtime.
   - Acceptance rg line: exits 1.
   - No `tests/` file in the unit diff.

6. Paths outside the worktree: as in the header line. The tree's `target/` was discarded with `worktree discard-cache` (865 MiB). Cargo time about 1.6 min, wall time about 25 min.

```findings
[
{"file":"crates/inference-testkit/src/graph/nodes.rs","line":151,"category":"mutant","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"send_unless lost its llm-feature caller in the move, so clippy -p inference-testkit --features llm,run -D warnings fails with dead_code; gate it on feature stt or retire it"},
{"file":"crates/inference-pipeline/src/nodes/tts.rs","line":621,"category":"judgement","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the testkit's MockSink became public pipeline API as PlaybackSink, so TtsNode::playing acknowledges playback no device performed under a name that hides it is a double"},
{"file":"crates/inference-pipeline/src/nodes/mod.rs","line":630,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"the_model_nodes_and_their_log_live_in_the_pipeline asserts only trait bounds, empties and new()==default(), so it cannot fail for a behaviour defect"}
]
```
