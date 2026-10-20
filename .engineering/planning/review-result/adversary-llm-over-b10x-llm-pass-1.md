---
format: aep.planning-md/3
id: review-result:adversary-llm-over-b10x-llm-pass-1
kind: review-result
status: active
title: Adversary, story:llm-over-b10x-llm, pass 1
relations:
- reviews: story:llm-over-b10x-llm
revision: 1
---
unit: story:llm-over-b10x-llm
verdict: red
cases: executed 988→1002, red 8
origin: introduced 7 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory, <host path> (logs only; the mutant copy and its target/ are deleted)
needs-coordinator: none. The planning-store edit in the tree (story revision 4→5, `## llm gaps`) was not written by me.

The tree covered is base 15a3349 plus the uncommitted change. "Before" is the "after" count minus my 14 added cases. I made no run with my files deselected.

**1. What I touched.** `git --no-pager diff --stat 15a3349` shows 33 tracked files, all from the unit or the coordinator. I edited no tracked file. My changes are three new, untracked test files:
- `crates/inference-llm/tests/adversary1_turn_sequencing.rs` (10 cases)
- `crates/inference-testkit/tests/adversary1_llm_node_turns.rs` (2 cases)
- `crates/inference-runtime/tests/adversary1_llm_rounds.rs` (2 cases)

**2. Cases added, each file run alone first** (logs `red-llm.log`, `red-testkit.log`, `red-runtime.log`)

| case | asserts | now |
|---|---|---|
| `a_turn_ending_in_a_call_and_no_text_leaves_its_round_answerable` | proposal taken while Active, `integrate` Ok | RED: `left: Completed right: Active`, events seen `[Started, Completed]` |
| `a_turn_whose_end_arrives_after_its_last_delta_leaves_its_round_answerable` | same; the turn's end is fed after its last delta | RED: `left: Completed right: Active`, events seen `[Completed]` |
| `a_tool_calling_adapter_that_does_not_stream_can_answer_its_round` | same; adapter claims `tool_calling` only | RED: `left: Completed right: Active` |
| `tool_id_resolves_only_names_tool_name_gives` | an upper-case name gives None | RED: `left: Some(ToolId(..)) right: None` |
| `the_model_node_asks_its_model_once_per_input_and_answers_each` | sink gets `["re:hello","re:world"]` | RED: `left: ["unprompted", "re:world"]`; the model was asked `[[], [AssistantText "unprompted", UserText "hello", UserText "world"]]` |
| `a_failing_turn_is_not_a_completed_run_with_the_failure_unseen` | the run is not Completed, or it records GenerationCancelled, or the node fails | RED: `run Completed, committed downstream [], GenerationCancelled recorded false, GenerationCompleted recorded false, node failure None, dropped inputs ["world"]` |
| `every_call_of_a_second_round_of_two_reaches_the_authority` | 4 authority decisions | RED: `left: [Berlin, Paris]`, model requests 2 |
| `a_call_ending_a_turn_with_no_text_reaches_the_authority` | 1 decision when the reply is empty | RED: `left: 0 right: 1` |
| 6 guard cases in the llm file: delta after supersede, accepted delta untaken at cancel, call fragments, empty delta, round answered in part, round answered out of order | (each case's name) | green |

The first run of the runtime file was red for a reason in my own test: it declared the tool twice, so the node answered every call "denied". I fixed the test, and the red output above is from the rerun.

**3. Package gate runs, after my cases existed** (each command run alone, exit status shown):
- `cargo test -p inference-llm --all-features --no-fail-fast`: EXIT=101, passed 30, failed 4 (my 4)
- `cargo test -p inference-testkit --all-features --no-fail-fast`: EXIT=101, passed 819, failed 2 (my 2)
- `cargo test -p inference-runtime --all-features --no-fail-fast`: EXIT=101, passed 145, failed 2 (my 2)
- `cargo clippy -p <each of the three> --all-targets --all-features -- -D warnings`: EXIT=0 for all three
- `cargo fmt -p <each of the three> --check`: EXIT=0 for all three

**Mutants.** I applied three mutants together to a scratch copy of `lib.rs`:
- line 931: the empty-delta filter removed
- line 777: the "every call of the round answered" check removed
- line 860: `integrate` accepts only the first awaited call

I then ran `cargo test -p inference-llm -p inference-testkit -p inference-runtime -p inference-facade --all-features` on that copy: passed 1036, failed 10. Every failure is one of my cases. The unit's own suites stayed green under all three mutants. My guards `an_empty_delta_is_no_segment`, `a_round_answered_in_part_asks_no_turn` and `a_round_answered_out_of_order_gives_a_valid_request` catch them.

**4. Findings** (each with what was measured, then what reaches it)

**F1. blocker, NEEDS-CHANGE, introduced. `crates/inference-llm/src/lib.rs:782`**
- Measured: with no turn running and calls still unanswered, `poll_event` completes the generation. The poll that ends a turn reaches that branch at once unless the same poll queued an event. So no proposal can be answered when the turn ends with calls and no text, when its end arrives in a later poll than its last delta, or when the adapter does not stream. Shown by 3 llm cases and 1 runtime case. With the mock, an empty reply now gets 0 decisions; at base it got 1, because the base mock emitted a segment even for empty text.
- What reaches it: any `llm` model that makes a tool call without prose, and any model reading a stream. llm-chat's `ChatClient::turn` emits per SSE event and returns the outcome only after the stream ends (`git show 0.7.0:crates/llm-chat/src/client.rs`, lines 80-105).
- Fix: while calls are awaited, `poll_event` returns Pending and `integrate` wakes it. Completing with calls unanswered becomes an explicit handle call, used by the node's exhausted path and end of input.

**F2. warning, NEEDS-CHANGE, introduced. `crates/inference-testkit/src/llm/node.rs:621`**
- Measured: `answer()` delivers only up to the first SegmentCommitted, and `take_proposal` (lib.rs:836) returns None while events remain undelivered. A turn with two deltas that ends with a round therefore never sends that round: 2 decisions instead of 4. The generation completes with 2 calls unanswered.
- What reaches it: `MockGenerator::proposing_round(_, n ≥ 2)`, which no in-repo caller uses, and any model that streams more than one delta in a tool turn.

**F3. warning, NEEDS-CHANGE, introduced. `crates/inference-testkit/src/llm/node.rs:1159`**
- Measured: the node's first turn asks the model a request with no items before any input arrives. Its answer goes downstream as the reply to input 1, and input 1 is folded into the turn for input 2.
- What reaches it: the node documents "any `inference-llm` Generator". The mock with an empty script and `Shouting` in `llm_model_node.rs` both answer nothing to an empty request, which hides the problem. The unit declared this deviation.

**F4. warning, NEEDS-CHANGE, introduced. `crates/inference-llm/src/lib.rs:977`**
- Measured: `turn_failed` drops the GenerationCancelled and SegmentDiscarded events. `GenerationEvent` has no cancelled variant, and the node never reads `failure()`. Result: the run ends Completed, downstream gets no terminal generation event, and later input is dropped.
- What reaches it: any `llm` turn error, including errors raised after output was streamed (llm-chat `client.rs` lines 94-104 at 0.7.0). The mock never fails a turn.

**F5. note, INFEASIBLE, introduced. `crates/inference-llm/src/tools.rs:41`**
- Measured: `tool_id` resolves an upper-case name, contrary to its doc ("none otherwise").
- What reaches it: nothing found. llm's wires validate the outcome against the published tool names, and the mock uses the canonical name.

**F6. note, CONFIRMED, introduced. `docs/design/inference.md:371`**
- The design states "`llm` never revises a delta it streamed" as a fact.
- The crate doc (lib.rs:26) and the unit's gap list call this unstated in `llm`, an `llm` gap. A turn that fails after its deltas cancels segments that were already committed (F4).

**F7. note, CONFIRMED, introduced. `crates/inference-llm/src/lib.rs:931`**
- Three mutants (lines 931, 777, 860) survive all four packages' suites. My guard cases now kill them.

**5. Attacked and not broken**
- A delta fed after a supersede or cancel is not delivered.
- A turn ending after the generation ended adds nothing.
- Tool-call announcements, argument fragments and reasoning produce neither text nor proposals.
- A call id `llm` did not issue, or one answered twice, gets NotAwaited.
- Answers out of order give a request that passes llm `validate()`.
- `ToolOutcome` encodes failure and rejection under different keys (`failure` / `rejected`) with ESS snake_case names.
- No `llm` type is re-created, and `Cargo.lock` adds only `b10x-llm-core`.
- The feedback-loop bound, interruption and cascade suites (runtime, 145 passed) and the facade suite (green on the mutant copy only) still pass.
- `max_concurrency ≤ 0` is refused at adapter declaration, so the `usize::MAX` fallback is never reached.

**6. Paths written outside the worktree**
- <host path> holding 10 logs: red-llm.log, red-testkit.log, red-runtime.log, gate-test-inference-{llm,testkit,runtime}.log, gate-clippy-inference-{llm,testkit,runtime}.log, mutant-combined.log
- <host path> (deleted)

**7. Findings block**
```findings
[
 {"file": "crates/inference-llm/src/lib.rs", "line": 782, "category": "acceptance", "severity": "blocker", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "poll_event completes the generation while tool calls are unanswered, so a round cannot be answered when the turn ends with calls and no text, when its end arrives after its last delta, or when the adapter does not stream"},
 {"file": "crates/inference-testkit/src/llm/node.rs", "line": 621, "category": "boundary", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "answer() stops at the first committed segment, so the calls of a turn that streamed more than one delta are never proposed"},
 {"file": "crates/inference-testkit/src/llm/node.rs", "line": 1159, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "the node's first turn asks the model an empty request, whose answer is sent as the reply to the first input while that input is never answered on its own"},
 {"file": "crates/inference-llm/src/lib.rs", "line": 977, "category": "contract-drift", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "a failing turn cancels the generation without any event, and the model node ends the run Completed with no GenerationCancelled and later input dropped"},
 {"file": "crates/inference-llm/src/tools.rs", "line": 41, "category": "boundary", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "tool_id resolves upper-case names that tool_name never gives, contrary to its doc; llm wires refuse unpublished names first"},
 {"file": "docs/design/inference.md", "line": 371, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "the design states as fact that llm never revises a delta, which the crate doc and the gap list call unstated in llm"},
 {"file": "crates/inference-llm/src/lib.rs", "line": 931, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "mutants at lines 931, 777 and 860 survive the unit's suites in four packages; the adversary guard cases kill them"}
]
```