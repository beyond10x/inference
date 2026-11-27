---
format: aep.planning-md/3
id: review-result:sdk-one-call-adversary-2
kind: review-result
status: active
title: Adversary pass 2 on the one-call SDK functions
relations:
- reviews: story:sdk-one-call
revision: 1
---
Adversary, pass 2 (the last), on story:sdk-one-call, 2026-11-24, as the adversary returned it; `<scratch>` is the pass's scratch directory outside the repository.

unit: story:sdk-one-call
verdict: red
cases: executed 461→489, red 8
origin: introduced 4, pre-existing 0, undecided 2
wrote-outside-worktree: 12 log files under <scratch>/ (listed in part 6)
needs-coordinator: yes

Covers the working tree of `impl/sdk-one-call` at head `9daf34cf34e8cbb2fffbc1d451ea8936f8795431` plus my two untracked case files. `executed`: `inference` 303→323 and `inference-pipeline` 158→166; "before" is the implementor's claim. `red 8` is each case file run alone; the suite run had 6 red, because two of the timing-dependent cases went green under the suite's load. Lease `w2-u1-adversary-2` released (`session-end` exit 0).

**What stands against the merge:** a model reply that ends with three or more tool calls intermittently hangs `Chat::send` and then `Chat::close` (finding 1). This was the last pass, so it goes back with a blocker open.

## 1. What I touched

`git --no-pager diff --stat`: empty (no tracked file changed).

`git status --short`:
```
?? crates/inference-pipeline/tests/adversary2_w2u1_tool_executor_panic.rs
?? crates/inference/tests/adversary2_w2u1_one_call.rs
```
Both are new test files, uncommitted. No implementation file and no existing test was edited; nothing under `.engineering/`; no commit, stash, branch or push.

## 2. Cases added

### `crates/inference/tests/adversary2_w2u1_one_call.rs` (20 cases: 8 red, 12 green)

Command, the file alone, before any suite run:
`CARGO_INCREMENTAL=0 cargo nextest run -p inference --features testing --test adversary2_w2u1_one_call --no-fail-fast`
Result: `Summary [   6.057s] 20 tests run: 12 passed, 8 failed, 0 skipped`, exit 100 (`<scratch>/run6-sdk-cases.log`).

Four cases run over 5 s only when they are red (each stall costs a 1 s wait plus a 1 s close wait); green they end under 0.1 s:
- `a_chat_answers_a_round_of_three_calls_in_each_of_thirty_conversations`: 6.0 s
- `a_chat_run_runs_every_call_of_a_round_of_three_in_each_of_thirty_runs`: 6.1 s
- `a_chat_answers_a_round_of_four_calls_in_each_of_thirty_conversations`: 4.1 s to 6.0 s
- `a_text_agent_says_the_reply_to_a_round_of_four_calls_or_tells_an_error`: 4.0 s

Red cases, output verbatim from that run:

**R1 `a_chat_answers_a_round_of_three_calls_in_each_of_thirty_conversations`** (finding 1). The model's first reply ends with three calls; `Chat::send` should return the reply carrying three answers.
```
thread 'a_chat_answers_a_round_of_three_calls_in_each_of_thirty_conversations' (2828578) panicked at crates/inference/tests/adversary2_w2u1_one_call.rs:287:5:
the first 3 of at most 30 conversations that did not answer a round of three calls:
conversation 6: send did not return within 1s; the executor ran 2 of 3 calls; a close then came to Err(Elapsed(()))
conversation 8: send did not return within 1s; the executor ran 2 of 3 calls; a close then came to Err(Elapsed(()))
conversation 10: send did not return within 1s; the executor ran 2 of 3 calls; a close then came to Err(Elapsed(()))
```

**R2 `a_chat_answers_a_round_of_four_calls_in_each_of_thirty_conversations`** (finding 1).
```
thread 'a_chat_answers_a_round_of_four_calls_in_each_of_thirty_conversations' (2828596) panicked at crates/inference/tests/adversary2_w2u1_one_call.rs:299:5:
the first 3 of at most 30 conversations that did not answer a round of four calls:
conversation 2: send did not return within 1s; the executor ran 2 of 4 calls; a close then came to Err(Elapsed(()))
conversation 4: Layer: the session's run failed: FailureReason { node_id: Some(NodeId(d83d7177-5410-4ee7-9b49-76c12d0d3640)), detail: "the edge is full and fails on overload; event 7821f896-4fe2-4a3b-b6b7-ceb1873de0ad was not sent" }; the executor ran 2 of 4 calls
conversation 16: send did not return within 1s; the executor ran 2 of 4 calls; a close then came to Err(Elapsed(()))
```

**R3 `a_text_agent_says_the_reply_to_a_round_of_four_calls_or_tells_an_error`** (finding 1). Same round through `Agent::text().tool(..)`: the stream goes silent, with no `Event::Error`, and `Agent::close` does not return.
```
thread 'a_text_agent_says_the_reply_to_a_round_of_four_calls_or_tells_an_error' (2828607) panicked at crates/inference/tests/adversary2_w2u1_one_call.rs:357:5:
conversation 6: within 3s the stream neither said the reply nor told an error; the executor ran 2 of 4 calls; a close then came to Err(Elapsed(())); the events were:
Started
Said the weather
Usage
ToolCalled {"city":"Berlin"}
ToolCalled {"city":"Paris"}
ToolCalled {"city":"Rome"}
ToolCalled {"city":"Oslo"}
```

**R4 `a_chat_run_runs_every_call_of_a_round_of_three_in_each_of_thirty_runs`** (finding 1). Direct `Instantiation::open`, session read: the close returns but the run's driver never ends.
```
thread 'a_chat_run_runs_every_call_of_a_round_of_three_in_each_of_thirty_runs' (2828734) panicked at crates/inference/tests/adversary2_w2u1_one_call.rs:887:5:
the first 3 of at most 30 runs that left a call of the round out:
run 0: the executor ran 2 of 3 calls within 1s, 11 events read; close came to Ok(Ok(())); the driver came to Err(Elapsed(()))
run 3: the executor ran 2 of 3 calls within 1s, 11 events read; close came to Ok(Ok(())); the driver came to Err(Elapsed(()))
run 5: the executor ran 2 of 3 calls within 1s, 11 events read; close came to Ok(Ok(())); the driver came to Err(Elapsed(()))
```

**R5 `a_chat_whose_model_calls_its_tool_once_a_turn_answers_six_turns`** (finding 2). A model that calls its tool once per user turn; six turns on one `Chat`. The control with four turns is green.
```
thread 'a_chat_whose_model_calls_its_tool_once_a_turn_answers_six_turns' (2828616) panicked at crates/inference/tests/adversary2_w2u1_one_call.rs:556:5:
assertion `left == right` failed: the chat answered that many of 6 turns of one call each before a send failed; the executor ran 4 calls and the view gained 1 sessions
  left: (4, Some("Closed: closed: the chat's session ended before the reply was whole, as when the model's tool calls reach the bound of the tool loop; the next text starts a new conversation"))
 right: (6, None)
```

**R6 `every_tool_called_of_a_text_agent_carries_the_arguments_of_its_call`** (finding 3). A round of two calls, 40 conversations; the executor received the right arguments every time.
```
thread 'every_tool_called_of_a_text_agent_carries_the_arguments_of_its_call' (2829815) panicked at crates/inference/tests/adversary2_w2u1_one_call.rs:228:5:
31 of 40 conversations told a call without its arguments; each should tell [{"city":"Berlin"},{"city":"Paris"}]:
round 4: [null,null]
round 5: [null,null]
round 6: [null,null]
round 7: [null,null]
round 8: [null,null]
round 9: [null,null]
round 10: [{"city":"Berlin"},null]
round 13: [{"city":"Berlin"},null]
round 14: [null,null]
round 15: [null,null]
round 16: [null,null]
round 17: [null,null]
round 19: [null,null]
round 20: [{"city":"Berlin"},null]
round 22: [null,null]
round 23: [null,null]
round 25: [null,null]
round 26: [null,null]
round 27: [null,null]
round 28: [null,null]
round 29: [null,null]
round 30: [null,null]
round 31: [null,null]
round 32: [null,null]
round 33: [null,null]
round 34: [null,null]
round 35: [null,null]
round 36: [null,null]
round 37: [null,null]
round 38: [null,null]
round 39: [null,null]
```

**R7 `every_tool_called_of_a_native_voice_agent_carries_the_arguments_of_its_call`** (finding 4). One call per conversation, 20 conversations. It fails there too, as the brief expected.
```
thread 'every_tool_called_of_a_native_voice_agent_carries_the_arguments_of_its_call' (2829809) panicked at crates/inference/tests/adversary2_w2u1_one_call.rs:406:5:
17 of 20 conversations told a call without its arguments:
round 0: [null]
round 1: [null]
round 2: [null]
round 4: [null]
round 5: [null]
round 6: [null]
round 7: [null]
round 8: [null]
round 9: [null]
round 11: [null]
round 13: [null]
round 14: [null]
round 15: [null]
round 16: [null]
round 17: [null]
round 18: [null]
round 19: [null]
```

**R8 `a_tool_that_awaits_the_applications_runtime_answers_its_call`** (finding 5). A tool whose future awaits `tokio::time::sleep`.
```
thread '<unnamed>' (2829688) panicked at crates/inference/tests/adversary2_w2u1_one_call.rs:634:17:
there is no reactor running, must be called from the context of a Tokio 1.x runtime

thread 'a_tool_that_awaits_the_applications_runtime_answers_its_call' (2829058) panicked at crates/inference/tests/adversary2_w2u1_one_call.rs:760:5:
assertion `left == right` failed: the reply carries the tool's answer
  left: "the weatherthe tool failed: the tool's executor panicked: there is no reactor running, must be called from the context of a Tokio 1.x runtime"
 right: "the weather{\"temperature\":21}"
```

Green cases in the same file (12):
- `a_chat_whose_model_calls_its_tool_once_a_turn_answers_four_turns`
- `a_chat_whose_tool_panics_without_a_message_returns_the_reply_to_the_failed_call`
- `a_round_whose_first_call_panics_still_runs_its_second`
- `a_confirmed_call_whose_tool_panics_is_answered_as_failed`
- `a_chat_run_whose_session_is_not_read_runs_the_calls_of_its_round`
- `a_chat_run_closed_with_two_calls_held_denies_both_and_ends`
- `a_text_agent_holding_two_calls_closed_twice_at_once_closes`
- `a_text_agent_dropped_with_a_call_held_closes_its_session`
- `a_chat_closed_while_its_tool_runs_returns_once_the_tool_answered`
- `a_chat_dropped_while_its_tool_runs_closes_its_session`
- `a_voice_whose_synthesiser_fails_mid_way_returns_an_error_and_closes_its_session`
- `speak_with_over_a_synthesiser_that_fails_mid_way_returns_and_lets_go_of_the_speaker`

### `crates/inference-pipeline/tests/adversary2_w2u1_tool_executor_panic.rs` (8 cases, all green)

Command: `CARGO_INCREMENTAL=0 cargo nextest run -p inference-pipeline --all-features --test adversary2_w2u1_tool_executor_panic --no-fail-fast`
Result: `Summary [   0.107s] 8 tests run: 8 passed, 0 skipped`, exit 0 (`<scratch>/run6-pipeline-cases.log`).

- `a_call_whose_tool_panicked_as_it_was_called_is_not_attempted_again_under_three_attempts`
- `a_call_whose_tools_future_panicked_is_not_attempted_again_under_three_attempts`
- `a_call_that_failed_transiently_and_then_panicked_ends_failed_after_two_attempts`
- `a_transient_failure_is_attempted_again_and_its_later_answer_is_the_result`
- `a_transient_failure_past_the_limit_fails_the_call_with_it`
- `a_panic_that_carries_no_message_fails_its_call_and_the_node_serves_the_next`
- `a_future_that_panics_on_its_second_poll_fails_its_call_and_the_node_serves_the_next`
- `a_tool_that_never_answers_and_one_that_panics_each_fail_their_call_alone` (100 ms deadline)

The first three cover the retry guard at `crates/inference-pipeline/src/nodes/tool_executor.rs:496`, which the unit's own two tests do not exercise (both use the default policy of one attempt).

## 3. Suite runs, after the cases existed

One lane at a time; every step far inside 120 s, no test near 60 s.

| Command | Result | Exit | Log |
|---|---|---|---|
| `CARGO_INCREMENTAL=0 cargo nextest run -p inference --all-features --no-fail-fast` | `Starting 323 tests across 24 binaries (1 binary skipped via profile.default.default-filter)`; `Summary [   4.194s] 323 tests run: 317 passed, 6 failed, 0 skipped` | 100 | `<scratch>/suite-inference.log` |
| `CARGO_INCREMENTAL=0 cargo nextest run -p inference-pipeline --all-features --no-fail-fast` | `Summary [   3.137s] 166 tests run: 166 passed, 0 skipped` | 0 | `<scratch>/suite-inference-pipeline.log` |
| `CARGO_INCREMENTAL=0 cargo nextest run -p inference-runtime --all-features --no-fail-fast` | `Summary [   2.358s] 287 tests run: 287 passed, 0 skipped` | 0 | `<scratch>/suite-inference-runtime.log` |
| `CARGO_INCREMENTAL=0 cargo nextest run -p inference-testkit --all-features --no-fail-fast` | `Starting 1389 tests across 66 binaries`; `Summary [  12.213s] 1389 tests run: 1389 passed, 0 skipped` | 0 | `<scratch>/suite-inference-testkit.log` |

`inference-testkit` is the only other package matching `rg -l "Recipe::chat|Agent::text" examples adapters crates` (`crates/inference-testkit/tests/target/sdk.rs`); no example or adapter matches.

The `inference` lane's failures, verbatim:
```
     Summary [   4.194s] 323 tests run: 317 passed, 6 failed, 0 skipped
        FAIL [   0.018s] ( 62/323) inference::adversary2_w2u1_one_call a_chat_whose_model_calls_its_tool_once_a_turn_answers_six_turns
        FAIL [   0.017s] ( 68/323) inference::adversary2_w2u1_one_call a_tool_that_awaits_the_applications_runtime_answers_its_call
        FAIL [   0.044s] ( 72/323) inference::adversary2_w2u1_one_call every_tool_called_of_a_native_voice_agent_carries_the_arguments_of_its_call
        FAIL [   0.076s] ( 79/323) inference::adversary2_w2u1_one_call every_tool_called_of_a_text_agent_carries_the_arguments_of_its_call
        FAIL [   2.071s] (319/323) inference::adversary2_w2u1_one_call a_chat_answers_a_round_of_four_calls_in_each_of_thirty_conversations
        FAIL [   4.032s] (323/323) inference::adversary2_w2u1_one_call a_text_agent_says_the_reply_to_a_round_of_four_calls_or_tells_an_error
error: test run failed
```
- Every failure is one of my cases; no existing test failed.
- All 23 pass-1 cases passed: 22 in `adversary_w2u1_one_call`, 1 in `adversary_w2u1_one_call_send`.
- R1 and R4 (rounds of three) were green in this run: 30 of 30 conversations answered under the suite's load. The stall is a race, and its rate depends on load.
- R2 stalled in 1 of 30 conversations here: `conversation 3: send did not return within 1s; the executor ran 2 of 4 calls; a close then came to Err(Elapsed(()))`.

## 4. Findings

| # | file:line | Finding | Verdict | Origin | Severity | What reaches it |
|---|---|---|---|---|---|---|
| 1 | `crates/inference/src/recipe/topology.rs:117` | A model reply ending with three or more tool calls intermittently hangs `Chat::send` and then `Chat::close`; a text agent goes silent with no `Event::Error` and `Agent::close` hangs; a direct run's driver never ends. | NEEDS-CHANGE | introduced | blocker | `Chat::tool` and `Agent::text().tool(..)` with a model that proposes three or more calls in one reply and a tool that answers at once. |
| 2 | `crates/inference/src/recipe/topology.rs:114` | The fifth tool call of a chat's whole conversation ends its session with `Error::Closed` and drops the history. | NEEDS-CHANGE | introduced | warning | `Chat::tool` or `Agent::text().tool(..)` in any conversation whose model calls tools five times in total, for example one call in each of five turns. |
| 3 | `crates/inference/src/agent.rs:484` | `Event::ToolCalled.arguments` of a text agent is `null` in most conversations. | CONFIRMED | introduced | warning | `Agent::text().tool(..)`, the path `Chat`'s doc sends callers to for confirming calls, when the caller reads the arguments from `ToolCalled`. |
| 4 | `crates/inference-pipeline/src/nodes/duplex.rs:22` | The same `null` arguments on `native_voice`. | CONFIRMED | undecided | note | `Agent::voice().tool(..)` over a speech binding. |
| 5 | `crates/inference-pipeline/src/nodes/tool_executor.rs:466` | A tool whose future awaits anything of Tokio panics "there is no reactor running"; the model is told the tool failed. | CONFIRMED | undecided | warning | Any `#[inference::tool] async fn` that awaits a Tokio timer, socket or HTTP client, through `Chat::tool` or `AgentBuilder::tool`. |
| 6 | `CHANGELOG.md` | `Unreleased` still has no entry for the unit. | CONFIRMED | introduced | note | The release notes; carried from pass 1. |

Detail per finding:

1. **Stall on a round of three or more calls.**
   - Measured: R1 to R4. One conversation returned the cause as an error: "the edge is full and fails on overload; event ... was not sent".
   - Rate: 10 to 30 % of conversations with the case file run alone, 1 of 30 under the suite's load.
   - Mechanism: the tool-loop edges have capacity 4 under `OverloadPolicy::Fail` (`topology.rs:29`, `:110-118`), and the executor sends two envelopes per call (Started, then the result) on `outcomes→tool_result`. That the fifth envelope is the one refused is my reading of `crates/inference-runtime/src/queue.rs:459` and `tool_executor.rs:559-574`, `:659-662`; "ran 2 of N" in every stalled conversation fits it.
   - A round of two never stalled: 40 of 40 conversations in R6 got their reply, and pass 1 only tried rounds of two.
   - Origin: `chat` has no tool loop at base (`git show <base>:crates/inference/src/recipe/topology.rs`, lines 46-62). Base wires `native_voice` with the same capacity and policy; I could not attack that path because the speech mock proposes one call per reply.
   - Not measured: a real provider. No network was allowed, so the size of the window outside the mocks is unknown.

2. **The bound of four counts the whole conversation.**
   - Mechanism: the authority counts proposals per generation (`crates/inference-pipeline/src/nodes/tool_authority.rs:725-731`), and the model node runs one generation per run (`crates/inference-pipeline/src/nodes/llm.rs:44`).
   - The docs do not say what the bound counts: `crates/inference/src/recipe.rs:82-84` ("may return a call to the model: 4"), `crates/inference/src/chat.rs:164-166`, and the unit's commit message ("bounded at 4 rounds").
   - Consequences: a single round of five calls also exceeds it, and the brief's "100 tool rounds in one conversation" cannot happen.
   - Severity: I put warning because the failure is told as an error that names the bound. Whether a chat that "keeps its history" may lose it at the fifth tool call is the coordinator's decision.

3. **`null` arguments on a text agent.**
   - Mechanism: the correction sends each call on `tool_events` before the authority takes it (`crates/inference/src/recipe/run.rs:962-971`), and the pump reads the arguments from the authority's record (`agent.rs:482-486`).
   - `crates/inference/src/event.rs:69` says "when the agent's tool authority holds the call", so `null` is inside the letter of the doc; which of the two a caller gets is a race.
   - `ConfirmationRequested` does carry the arguments.
   - The implementor named this and did not test it.

4. **The same on `native_voice`.** `agent.rs:482-490` is identical at base, and the speech node sends the event before the call leaves on `tool_call`. I did not run it against base, so the origin stays undecided; by reading it is pre-existing, as the brief says.

5. **Tool futures are polled without a runtime.**
   - The executor node runs on its own thread (`crates/inference-pipeline/src/nodes/mod.rs:299-301`) and polls the tool's future there.
   - `docs/design/sdk.md` §5 defines tools as `async fn` and says nothing about the runtime.
   - By reading, the thread and the polling are the same at base, where the panic would have ended the node; the correction turns it into a failed call. Not run against base, so undecided.
   - `Chat::tool` is a new way to reach it.

6. **Changelog.** `CHANGELOG.md` is not in `git diff <base>...HEAD --stat`. The unit's brief keeps that file with the coordinator, and the unit's draft exists as `changelog.md` in the unit's scratch directory. The draft does not say what the bound of finding 2 counts.

### Pass-1 findings

| Pass-1 finding | Status |
|---|---|
| 1, `topology.rs:64`: text agent never yields `ConfirmationRequested` | Resolved. Pass 1's cases are green; `a_confirmed_call_whose_tool_panics_is_answered_as_failed` and `a_text_agent_holding_two_calls_closed_twice_at_once_closes` read the requests. |
| 2, `agent.rs:482`: no `ToolCalled` for a text agent | Resolved as to the event. Its arguments are finding 3. |
| 3, `chat.rs:236`: `Chat::send` never returns when the tool panics | Resolved for the panic (pass 1's cases, my 8 node cases, 3 SDK cases). The same symptom is reachable through finding 1 (R1, R2). |
| 4, `chat.rs:298`: `Chat::close` does not return after the panic | Resolved for the panic. The same symptom is reachable through finding 1 ("a close then came to Err(Elapsed(()))" in R1, R2). |
| 5, `run.rs:146`: a chat run does not end on close with a held call | Resolved: `a_chat_run_closed_with_two_calls_held_denies_both_and_ends` and pass 1's own case are green. |
| 6, `stack.rs:1475`: `stack.rs` and `recipe.rs` outside the story's scope list | Resolved, not a defect: the unit brief's file assignment names both as the unit's "where the unit needs them". |
| 7, `CHANGELOG.md` | Carried as finding 6; no case. |

Pass 1's file `crates/inference/tests/adversary_w2u1_one_call.rs` did not exist at `54d2130d5` (it was untracked then), so `git diff -w` against that commit shows the whole file and cannot prove "whitespace only". What I could check:
- Every line number and message quoted in pass 1's report matches the committed file up to `:409`.
- The later shifts (`:797→:805`, `:866→:878`) fit three broken call chains.
- The file holds 22 test attributes, the number pass 1 reported.

## 5. Attacked and could not break

- Panic with a payload that is neither `&str` nor `String`: the call fails, the next call is served.
- A future that panics on its second poll: the call fails, the next call is served.
- A panicked call under `RetryPolicy::up_to(3)`: attempted once. A transient failure is still retried and still fails at the limit, so the base's retry meaning holds.
- A tool that never answers beside one that panics, with a 100 ms deadline: each fails its own call.
- A panic in the first call of a round: the second call still runs, and `Chat::send` returns the reply.
- A panic in a confirmed call: answered as failed.
- `Closing` on chat: close with two calls held denies both and ends; two closes at once; drop with a call held; close and drop while a call executes.
- A chat run whose session is never read: its round of four calls still ran. This is a single conversation and is exposed to finding 1's race; it passed in every run.
- A synthesiser that panics at its third frame: `Voice::synthesise` returns an error and the session closes; `speak_with` returns and releases the speaker.
- Spans of an earlier `Transcriber` call in a later one: the existing `a_transcriber_returns_the_scripted_spans_committed_and_in_order` (`crates/inference/tests/oneshot.rs:188`) already calls twice and asserts exactly the scripted spans. No new case.

Not attacked, or by reading only:
- `Replying` on runs that are not a chat: no case written, although the brief asked for one. By reading, `Replying` wraps only the model node of non-cascade recipes (`crates/inference/src/recipe/run.rs:364-389`) and `native_voice` has no such node; the existing cascade and `native_voice` tests are green.
- The mutant "drop `!panicked &&`" at `tool_executor.rs:496` was not run on a copy; my first three node cases would catch it by reading.
- A panic payload whose own drop panics.
- A synthesiser that fails by returning an error (only a panic was tried).
- `exposure_capacity(1)`.
- A round of several calls on `native_voice`.
- Unwind safety of the executor's state: by cases only (the node serves the next call and each call is measured once), not by an audit of every lock.

## 6. Paths written outside the worktree

All under `<scratch>/` (the `adversary-2` scratch directory of the brief):
- `<scratch>/run1-pipeline-cases.log`
- `<scratch>/run1-sdk-cases.log`
- `<scratch>/run2-sdk-cases.log`
- `<scratch>/run3-sdk-cases.log`
- `<scratch>/run4-sdk-cases.log`
- `<scratch>/run5-sdk-cases.log`
- `<scratch>/run6-pipeline-cases.log`
- `<scratch>/run6-sdk-cases.log`
- `<scratch>/suite-inference.log`
- `<scratch>/suite-inference-pipeline.log`
- `<scratch>/suite-inference-runtime.log`
- `<scratch>/suite-inference-testkit.log`

Builds ran through the `cargo` shim on the shared pool; I pointed the compiler at no directory of my own. The logs contain absolute paths in their compile lines; do not publish them as they are.

Needs the coordinator:
- Finding 1 is a blocker found on the last pass.
- Finding 2 needs a decision: count the bound per reply, or document that it counts the conversation.
- Finding 6 is the coordinator's file.
- The two untracked case files are left for the coordinator to commit or drop. The four round cases are timing-dependent and would be flaky in a gate until finding 1 is fixed.

## 7. Findings block

```findings
- file: crates/inference/src/recipe/topology.rs
  line: 117
  category: concurrency
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a model reply that ends with three or more tool calls intermittently overflows a tool-loop edge of capacity 4 under the fail policy, after which Chat::send and Chat::close never return and a text agent goes silent without an Error event
- file: crates/inference/src/recipe/topology.rs
  line: 114
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the bound of 4 on chat's tool loop counts the tool calls of the whole conversation, so the fifth call ends the chat's session with Error::Closed and drops its history, while the docs do not say what the bound counts
- file: crates/inference/src/agent.rs
  line: 484
  category: concurrency
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: Event::ToolCalled of a text agent carries null arguments in most conversations, because the call is told on tool_events before the authority records it
- file: crates/inference-pipeline/src/nodes/duplex.rs
  line: 22
  category: concurrency
  severity: note
  verdict: CONFIRMED
  origin: undecided
  message: Event::ToolCalled of a native_voice agent carries null arguments in most conversations, by the same order of event and record; unchanged from base by reading, not run against base
- file: crates/inference-pipeline/src/nodes/tool_executor.rs
  line: 466
  category: boundary
  severity: warning
  verdict: CONFIRMED
  origin: undecided
  message: a tool whose future awaits a Tokio timer or socket panics with no reactor running, because the executor polls it on a thread without a runtime, and the model is told the tool failed
- file: CHANGELOG.md
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Unreleased has no entry for the consumer-visible behaviour changes the unit names
```
