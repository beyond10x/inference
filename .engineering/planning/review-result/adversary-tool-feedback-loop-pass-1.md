---
format: aep.planning-md/3
id: review-result:adversary-tool-feedback-loop-pass-1
kind: review-result
status: active
title: Adversary, story:tool-feedback-loop, pass 1
relations:
- reviews: story:tool-feedback-loop
revision: 1
---
unit: story:tool-feedback-loop, working tree of inference-impl-tool-feedback-loop (base 0bc17d0 plus the uncommitted change)
verdict: NEEDS-CHANGE
cases: executed 185→196, red 3
origin: introduced 7 / pre-existing 0 / undecided 0
wrote-outside-worktree: 9 paths, all under the assigned scratch (part 6)
needs-coordinator: the routing call on findings 1-3, and whether the declared bound must drive the authority (finding 8)

The pass is red: three new cases fail on the model node's tool loop, and four mutants survive the unit's own suite. The "before" count of 185 (runtime 112, pipeline 73) comes from the after-run with my three files' binaries left out of the sum, by `scratch/adversary-1/count.sh`.

**1. Diff stat**

`git --no-pager diff --stat` shows only the tracked change. It is identical to the implementor's (16 files, +1482 −201), so I touched no tracked file. Everything I added is untracked, and all of it is test files:
- `crates/inference-runtime/tests/adversary1_tool_feedback_loop.rs`
- `crates/inference-runtime/tests/adversary1_iteration_bound.rs`
- `crates/inference-pipeline/tests/adversary1_feedback_validation.rs`

No non-test path was touched.

**2. Cases added (each run alone before the suite)**

`adversary1_tool_feedback_loop.rs`, 3 cases, all red now:
- `a_drain_while_a_tool_round_is_in_flight_…` (:470) is red. The executor overruns a 400 ms deadline, and `RunRequests::drain()` is called once the call is authorized. First run: `left: Some(Failed) right: Some(Completed)`. A rerun with the reason: `run failure: Some("executor: the receiving side of the edge is gone; event … was not sent")`.
- `a_second_input_during_a_tool_round_…` (:494) is red. The second input is released while the first call is in flight; 2 calls are authorized but 1 outcome is reinjected: `left: 1 right: 2`, deltas `["looking up the weather in Berlin", "looking up and in Paris", "the tool failed: the call overran its deadline"]`.
- `an_input_after_the_bound_completed_the_generation_…` (:529) is red. Bound 1, 2 proposals; input 2 is released after `GenerationCompleted`: `left: Some("the generation is Completed, not Active, so nothing moved") right: None`.

`adversary1_iteration_bound.rs`, 3 guards, green:
- per-generation budget, with two generations interleaved;
- `denied` counts toward the bound and a `wrong-state` repeat does not;
- `NonZeroUsize::MAX` rejects nothing.

`adversary1_feedback_validation.rs`, 5 guards, green:
- a loop declared under another pipeline bounds nothing;
- a parallel unbounded edge keeps the cycle refused;
- of two cycles only the bounded one is admitted;
- a shared bounded edge plus a bounded self-loop validates;
- `DeclareFeedbackLoop` refusal order, 0/−1/`i64::MIN` refused, `i64::MAX` admitted.

**3. Suite (run after the cases existed)**

| command | exit |
|---|---|
| `cargo test -p inference-runtime --all-features --no-fail-fast` | 101: 118 executed, only my 3 cases fail (`test result: FAILED. 0 passed; 3 failed`, `error: 1 target failed`) |
| `cargo test -p inference-pipeline --all-features --no-fail-fast` | 0: 78 executed |
| `cargo clippy -p inference-runtime` / `-p inference-pipeline --all-targets --all-features -- -D warnings` | 0 / 0 |
| `cargo fmt -p inference-runtime` / `-p inference-pipeline --check` | 0 / 0 (I formatted only my three files) |

Mutants were run in a scratch copy against both packages' full suites. The copy and its `target/` (679M) are deleted.

| mutant | unit's suite (`tool_feedback_loop.rs` 3/3, `tool_nodes`, `validation`) | killed by |
|---|---|---|
| M1 `tool_authority.rs:396`, counter keyed per run instead of per generation | green | `the_bound_counts_per_generation…`, `a_denied_proposal…` |
| M2 the count moved after the tool check (denied proposals not counted) | green | `a_denied_proposal…` |
| M3 the count moved before the wrong-state check | green | `a_denied_proposal…` |
| M4 `validation.rs:81`, the `pipeline_id` filter dropped | green | `a_feedback_loop_declared_under_another_pipeline…` |

**4. Findings**

1. **Drain mid-round drops the round and fails the run.** Measured: the case at :470 is red. In `crates/inference-testkit/src/llm/node.rs:666-668`, `Step::Drain` completes the generation even while `awaiting` is set, and ends the exchange. The executor's result then hits a dropped `tool_result` receiver, and the run is Failed. What reaches it: `RunRequests::drain()`, a public caller API, after a source drain closes the prompt edge without a `stop`. Fix to name: on Drain with a call awaited, keep stepping until the outcome arrives and every `tool_result` edge has ended, as the end-of-input path does at :651-663.
2. **A second input during a round loses the first call's outcome.** Measured: the case at :494 is red. `node.rs:686-694` accepts a `TranscriptCommitted` while a call is awaited. `propose()` then overwrites `awaiting` (:516), and `outcome()` drops the first call's outcome (:558). What reaches it: any source sending two inputs within one tool round; the topology-A text pipeline does. Fix to name: defer inputs, or their proposals, until the awaited outcome is integrated, or track several awaited calls.
3. **An input after the termination rule fails the node.** Measured: the case at :529 is red. After `exhausted` completes the generation, `node.rs:690-692` offers the next input to the Completed generation, and the `?` fails the node. What reaches it: any later user input in the same run once the bound fired. Fix to name: drop or refuse input once the generation is no longer Active, without failing the node.
4. **Mutants M1-M4 survive the unit's tests.** Measured: the table in part 3. The authority's per-generation key, the counting order and check 5's pipeline filter are untested by `tool_feedback_loop.rs`, `tool_nodes.rs` and `validation.rs`. My guards now pin them. These four mutants are findings 4-7.
5. **(finding 8) The declared bound is decorative at runtime.** `FeedbackLoop.max_iterations` is read by nothing in the runtime. `ToolAuthority::with_iteration_bound` (`tool_authority.rs:167`) takes its own value, and `tests/tool_feedback_loop.rs:370-395` copies the declared bound into it by hand. A declaration of 5 with an authority built with 1 enforces 1, which differs from graph.yaml's "how many times one generation may go round the loop".
6. **(finding 9) The barred-proposal path cannot be reached from the node.** `node.rs:539-549` re-creates the basis span in the node's own turn and commits it, so `ToolCall::propose` never sees a Hypothesis span. The drop at :512-515 is dead code for the barred case. This is INFEASIBLE in the text pipeline: the node acts only on `TranscriptCommitted`.

**5. Attacked and not broken**
- The per-generation budget, N = 1 to 3 (unit's suite), and `NonZeroUsize::MAX` (my guard).
- A cycle bounded on an edge outside it, two cycles sharing one bounded edge, a cycle also closed by a parallel unbounded edge, a bounded self-loop, and a loop of another pipeline, as validated by `inference-pipeline` (live after the M4 filter).
- `DeclareFeedbackLoop` refusal order and the `max_iterations` boundaries; the Rust types match the graph.yaml names and fields.
- The six conform scenarios are not vacuous. They check refusal outcomes with error fields, the `FeedbackLoops` view row (`max_iterations` 1) and the invariant predicate. `ess specify validate` exits 0; synthesis gives 323 scenarios with only the 3 known ESS-SYNTH-001 refusals.
- The ordinary end-of-input path: the drain on the source's `stop` completes, as the unit's 3 cases show.

**6. Paths written outside the worktree** (all under `<unit scratch>/adversary-1/`):
- `run-mutant.sh`, `count.sh`
- `mutant-m1-per-run-counter.log`, `mutant-m2-denied-uncounted.log`, `mutant-m3-repeat-counted.log`, `mutant-m4-foreign-loop-bounds.log`
- `suite.json`, `gate-runtime-test.log`, `gate-pipeline-test.log`

`mutant/` (the copy with its `target/`) and the `mutant-*.full.log` files are already deleted. The lease is released (`session-end`).

```findings
- file: crates/inference-testkit/src/llm/node.rs
  line: 666
  category: concurrency
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: Step::Drain completes the generation with a tool call still awaited and drops tool_result, so the executor's send fails and the drained run ends Failed instead of reinjecting the in-flight outcome
- file: crates/inference-testkit/src/llm/node.rs
  line: 516
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: an input accepted while a call is awaited issues a second proposal that overwrites awaiting, so the first executed call's outcome is silently dropped and never reinjected
- file: crates/inference-testkit/src/llm/node.rs
  line: 690
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: an input arriving after the exhausted rejection completed the generation is offered to the Completed generation and fails the model node
- file: crates/inference-pipeline/src/nodes/tool_authority.rs
  line: 396
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: keying the iteration counter per run instead of per generation leaves the unit's suite green; caught only by adversary1_iteration_bound
- file: crates/inference-pipeline/src/nodes/tool_authority.rs
  line: 397
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: counting only proposals that pass the tool check (denied uncounted) leaves the unit's suite green; caught only by adversary1_iteration_bound
- file: crates/inference-pipeline/src/nodes/tool_authority.rs
  line: 388
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: counting a wrong-state repeat toward the bound leaves the unit's suite green; caught only by adversary1_iteration_bound
- file: crates/inference-pipeline/src/validation.rs
  line: 81
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: dropping the pipeline_id filter lets a feedback loop of another pipeline bound this pipeline's cycle with the unit's suite green; caught only by adversary1_feedback_validation
- file: crates/inference-pipeline/src/nodes/tool_authority.rs
  line: 167
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the declared FeedbackLoop.max_iterations is read by nothing at runtime; the authority's bound is a separate constructor value the test copies by hand
- file: crates/inference-testkit/src/llm/node.rs
  line: 539
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the node re-creates and commits its own basis span, so the barred-proposal drop the story requires cannot be reached and is untested
```