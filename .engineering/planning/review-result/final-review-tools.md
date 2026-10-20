---
format: aep.planning-md/3
id: review-result:final-review-tools
kind: review-result
status: active
title: Final adversary review, tools area
relations:
- reviews: story:tool-call-interruption
- reviews: story:cascade-read-only-tool
- reviews: story:voice-cascade-on-mocks
revision: 1
---
```
unit: tools and voice cascade (story:tool-call-interruption, story:cascade-read-only-tool, story:voice-cascade-on-mocks), tree inference-review-tools at c733f12 plus 4 untracked test files
verdict: NEEDS-CHANGE
cases: executed 242→248, red 3
origin: introduced 5 / pre-existing 0 / undecided 1
wrote-outside-worktree: 20 paths, all under <host path>
needs-coordinator: none
```

**Known item, settled: it is a test race, not a product race.** The one recorded failure is `cascade-chain/scratch/s1-runtime.log:142-145`. It failed at `adversary1_tool_retries.rs:576` with `left: []`, right `["inference.tools.ToolCallStarted"]`: the sink never recorded the Started event.
- The executor sends Started before the attempt is recorded (`tool_executor.rs:457-458`).
- A cancelled `TextSink` returns `None` without taking what is still queued (`graph/nodes.rs:166`).
- The test cancels at `:568` without waiting for the sink.
- Fix: `assert!(run.sink.wait_received(1, PATIENCE))` before `run.requests.cancel()`.
- I could not reproduce it in 1,900 reruns (300 single-test pinned to one CPU; 200 + 600 whole-binary pinned; 800 crowded onto two CPUs).
- The product race below would show the other way round, as `[Started, Failed]`, which is not what was recorded.

**1. `git --no-pager diff --stat`:** empty, because no tracked file changed. Four untracked files, all tests:
`crates/inference-pipeline/tests/adversary3_tool_executor_cancel.rs`, `crates/inference-pipeline/tests/adversary3_tool_executor_interruption.rs`, `crates/inference-runtime/tests/adversary2_cascade_cancel.rs`, `crates/inference-runtime/tests/adversary2_failed_node_attribution.rs`.

**2. Cases (each run alone first; red output verbatim)**

| case | asserts | now |
|---|---|---|
| `adversary3_the_answer_to_the_cancel_is_not_sent_after_the_cancel` | a cancel that lands while the node is sending Started lets nothing more out (100 trials) | red |
| `adversary3_a_call_of_an_interrupted_generation_starts_with_its_signal_fired` | the documented "started with its signal fired" behaviour, for both interruption kinds, on an executor without `answering_to` | green (guard) |
| `adversary2_a_cancel_ends_a_cascade_whose_egress_nobody_reads` | the cancel ends the run within 10 s when the caller holds egresses it never reads (300-word reply) | red |
| `adversary2_control_a_cancel_ends_a_cascade_whose_egresses_have_room` | the same run with a 3-word reply | green |
| `adversary2_draining_the_run_names_the_node_whose_exchange_failed` | a run failing while Draining names the node whose exchange failed | red |
| `adversary2_control_active_the_run_names_the_node_whose_exchange_failed` | the same failure while Active | green |

```
4 of 20 cancels let the node send after the cancel, e.g. Some(["inference.tools.ToolCallStarted", "inference.tools.ToolCallFailed"])
assertion `left == right` failed: the cancel ended the run within 10 s while the application held its egresses (last frame at 17600)
  left: None
 right: Some((Cancelled, true))
assertion `left == right` failed: the run names node Some(NodeId(c360e1d8-…)) (the first executor is c360e1d8-…): Err(NodeFailed(FailureReason { node_id: Some(NodeId(c360e1d8-…)), detail: "the receiving side of the edge is gone; event 7dcb72fd-… was not sent" })); the second executor's own failure: Some("the tool executor takes authorized calls; call 576dd296-… arrived Started")
```

At 100 trials the cancel case was red in every run: 22, 30 and 14 of 100 trials sent, then 34 in the suite run and 43 after the clippy edit.

**3. Suite (run after the cases existed):** `cargo test -p inference-pipeline -p inference-runtime --all-features --no-fail-fast` exited 101, with 245 passed and 3 failed (exactly the 3 red cases above). Without my 6 cases that is 242. `cargo clippy -p inference-pipeline -p inference-runtime --all-targets --all-features -- -D warnings` exited 0 after one `let_and_return` fix in my own file. `cargo fmt … --check` exited 0.

**4. Findings**

| # | file:line | verdict / origin | measured | what reaches it |
|---|---|---|---|---|
| F1 | `crates/inference-pipeline/src/nodes/tool_executor.rs:579-580` | NEEDS-CHANGE / introduced | `cancel()` fires the signal before it sets `state.cancelled` under the lock. An executor that answers the signal gets its ToolCallFailed sent after the cancel: 14–43 of 100 trials. With the two lines swapped, a scratch copy was green in 2 runs. At base `8c4da2c` the case is green: `send` checked the signal itself. | Any cancel that lands while the exchange thread is awake. My endpoint only widens a window that exists anyway. The documentation promises "the answer its executor gives after the cancel" is not sent. |
| F2 | `crates/inference-runtime/tests/adversary1_tool_retries.rs:568-576` | CONFIRMED / introduced | Test race, as settled above. | Whole-binary runs under load: 1 in 40 during the chains. |
| F3 | `crates/inference-testkit/src/stt/fan_out.rs:120` (also the plain `send().await` in the STT, TTS and LLM nodes) | NEEDS-CHANGE / introduced | A cancel does not end a send that waits for room. With 3 unread egresses the run is not Cancelled after 10 s; the control case passes. The STT and TTS module docs say "Cancelled, it sends nothing more." | `RunController::cancel` on the cascade while the caller holds egresses unread. The facade's `LiveSession::cancel` (`live.rs:792`) leaves the drive thread waiting until the application calls `receive` again. |
| F4 | `crates/inference-runtime/src/drive.rs:188` together with `controller.rs:1042` | CONFIRMED / undecided | A refusal is kept only while the run is Active. While Draining, the producer drains first, so the run fails naming the producer with "receiving side … gone", and the real failure is lost. | Any consumer that fails during the normal drain at the end of a run. At base the case hung once and failed once, with no message captured. |
| F5 | `crates/inference-pipeline/src/nodes/tool_executor.rs:383-385` | CONFIRMED / introduced | Deleting `signal.cancel()` here leaves 47 tool cases in 8 runtime test files and 12 pipeline lib tests green. My guard turns red: `left: [false, false]`, `right: [false, true]`. | An executor node built without `answering_to`, which is the default. |
| F6 | `crates/inference-testkit/src/llm.rs:240` | CONFIRMED / introduced | The tool set is offered only through `MockGenerator::start_offering`. `inference_llm::GenerationRequest` carries no tools, so Check 1 reads a mock-only record. | Any real adapter. The story body already lists this as a gap. |

The findings cover the tree `inference-review-tools` at `c733f12` plus the 4 untracked files.

**5. Attacked and could not break**
- The shared call record's locks: no nested locking, so no deadlock, and start versus reject are both made under the record lock.
- A superseded generation racing authorization, start and completion: covered by the record and the `not_reinjected` path.
- Two consumer failures at once, and a cancel during the early drain.
- The order in which the STT node sends the committed span before `TurnCompleted`.
- A pending receive future being dropped: the runtime queue keeps the waker.
- The fan-outs: no frame lost or duplicated, and `stop` is forwarded.

**6. Paths written outside the worktree** (all under `<host path>`):
- scripts: `loop.sh`, `loopall.sh`, `mutant-run.sh`
- logs: `build1.log`, `loop-cpu0.log`, `loopall-cpu0.log`, `loopall-2,3.log`, `loopall-4,5.log`, `loopall-6,7.log`, `loopall-8,9.log`, `crowd-1.log` … `crowd-8.log`, `mutant1.log`, `suite.log`
- The scratch copies `fixcopy`, `mutant1` and `basecopy` were each cleaned with `cargo clean` and removed.

**7.**
```findings
- file: crates/inference-pipeline/src/nodes/tool_executor.rs
  line: 579
  category: concurrency
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: cancel fires the call signal before recording the cancel under the send lock, so the executor's answer to the cancel is sent after it (14-43 of 100 widened trials)
- file: crates/inference-runtime/tests/adversary1_tool_retries.rs
  line: 568
  category: concurrency
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the test cancels without waiting for the sink to take ToolCallStarted, so a cancelled sink drops it and the assertion sees []
- file: crates/inference-testkit/src/stt/fan_out.rs
  line: 120
  category: concurrency
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the testkit voice nodes do not end a waiting send on cancel, so cancelling a cascade whose caller holds unread egresses never finishes
- file: crates/inference-runtime/src/drive.rs
  line: 188
  category: concurrency
  severity: warning
  verdict: CONFIRMED
  origin: undecided
  message: a consumer failing while the run is Draining makes the run fail naming its producer with an edge-closed error, and the real failure is lost
- file: crates/inference-pipeline/src/nodes/tool_executor.rs
  line: 383
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: deleting the signal fired for a call of an already-interrupted generation survives every existing tool case; the added guard catches it
- file: crates/inference-testkit/src/llm.rs
  line: 240
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: tool offering exists only on MockGenerator, and the inference-llm GenerationRequest carries no tool set, so check 1 reads a mock-only record
```