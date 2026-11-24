---
format: aep.planning-md/3
id: review-result:sdk-one-call-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the one-call functions
relations:
- reviews: story:sdk-one-call
revision: 1
---
Adversary pass 1 on story:sdk-one-call, 2026-11-24, as the adversary returned it; `<scratch>` is the pass's scratch directory outside the repository.

```
unit: story:sdk-one-call
verdict: red
cases: executed 278→301, red 5
origin: introduced 7, pre-existing 0, undecided 0
wrote-outside-worktree: 5 paths under <scratch> (listed in part 6)
needs-coordinator: yes
```

Covers head `54d2130d5d0c816b4619e575485004a3a13fbb5e` plus my two untracked test files. `<scratch>` is the scratch directory the brief names. `<before>` = 278 is the implementor's claim; I did not run the suite before my cases existed. Lease `w2-u1-adversary-1` is released (`session-end` exit 0). Nothing committed.

needs-coordinator, two things I cannot settle:
- The diff changes `crates/inference/src/recipe.rs` (+84) and `crates/inference/src/stack.rs` (+52), which the story's scope list does not name. Whether another unit of the wave touches them is yours to check.
- `CHANGELOG.md` `## [Unreleased]` is empty. `AGENTS.md:45` puts consumer-visible changes there when they land on `main`; the unit names several.

## 1. What I touched

`git --no-pager diff --stat`: empty (no tracked file changed).

`git status --short`:
```
?? crates/inference/tests/adversary_w2u1_one_call.rs
?? crates/inference/tests/adversary_w2u1_one_call_send.rs
```
Both are test files. No implementation file and no existing test was edited.

## 2. Cases added

Command, my file alone, before the suite (third run, after the last cases were added):
`CARGO_INCREMENTAL=0 cargo nextest run -p inference --features testing --test adversary_w2u1_one_call --no-fail-fast` → `22 tests run: 17 passed, 5 failed, 0 skipped`, exit 100. Log: `<scratch>/run3-cases.log`. No case runs over 5 s; each red case waits out its own 3 s limit.

Red now (5), all in `crates/inference/tests/adversary_w2u1_one_call.rs`:

**`a_text_agent_requests_the_confirmation_of_the_call_it_holds`** (:187). `Agent::text()` with a tool that requires confirmation must yield `Event::ConfirmationRequested` within 3 s, then run the call once confirmed.
```
thread 'a_text_agent_requests_the_confirmation_of_the_call_it_holds' (4092458) panicked at crates/inference/tests/adversary_w2u1_one_call.rs:216:9:
no ConfirmationRequested within 3s: the call is held and nothing tells its id; the events were [
    Started { ... },
    Said { text: "the weather in Berlin", ... },
    ...
]
```
(event dump shortened; full text in the log, lines 199-291)

**`a_text_agent_tells_the_call_of_its_tool`** (:246). `Agent::text()` with a plain tool: the executor runs the call, the reply carries its answer, and the stream holds an `Event::ToolCalled`.
```
thread 'a_text_agent_tells_the_call_of_its_tool' (4091209) panicked at crates/inference/tests/adversary_w2u1_one_call.rs:286:5:
the executor ran a call the stream never told: [
    Started { ... },
    Said { text: "the weather in Berlin", ... },
    ...
    Said { text: "{\"temperature\":21}", ... },
]
```
(shortened; log lines 22-125)

**`a_chat_whose_tool_panics_returns_from_its_send`** (:297). `Chat::send` returns within 3 s when the tool's executor panics.
```
thread '<unnamed>' (4091817) panicked at crates/inference/tests/adversary_w2u1_one_call.rs:130:9:
the tool has a defect
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

thread 'a_chat_whose_tool_panics_returns_from_its_send' (4091339) panicked at crates/inference/tests/adversary_w2u1_one_call.rs:308:5:
send did not return within 3s of its tool's panic
```

**`a_chat_whose_tool_panicked_closes_once_its_send_was_given_up`** (:319). After the caller gives the `send` up at 500 ms, `Chat::close` returns within 3 s.
```
thread '<unnamed>' (4092858) panicked at crates/inference/tests/adversary_w2u1_one_call.rs:130:9:
the tool has a defect
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

thread 'a_chat_whose_tool_panicked_closes_once_its_send_was_given_up' (4092459) panicked at crates/inference/tests/adversary_w2u1_one_call.rs:332:5:
close did not return within 3s
```

**`a_chat_run_closed_with_a_call_held_for_confirmation_ends`** (:346). One layer down: `Recipe::chat().tools([..]).instantiate(..)`, `Instantiation::open`, one committed text, the authority holds the call, then `session.close()` and reading to the end finish within 3 s. Same shape as `closing_with_a_held_call_unanswered_rejects_it_denied` in `crates/inference/tests/recipes.rs:1428` for `native_voice`.
```
thread 'a_chat_run_closed_with_a_call_held_for_confirmation_ends' (4091132) panicked at crates/inference/tests/adversary_w2u1_one_call.rs:409:5:
closing a chat session with a call held unanswered does not end within 3s
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

Green now (17 in that file, 1 in the other): listed in part 5. Two of them are controls for the red cases:
- `a_chat_run_closed_after_its_call_ran_ends` (:797): same harness as the last red case, tool needs no confirmation, close ends. So the red is the held call, not my harness.
- `a_chat_dropped_after_its_tool_panicked_closes_its_session` (:866): dropping the chat does close the session. Drop is the one way out of the two panic cases.

`crates/inference/tests/adversary_w2u1_one_call_send.rs`, one test, green: compiles only if the futures of the one-call functions and of the data methods of `Chat`, `Transcriber`, `Voice` are `Send` (`<scratch>/run2-send.log`, exit 0).

One correction in my own file: a first version of `a_voice_given_the_empty_text_...` asserted "no frame" and went red on the mock synthesiser's own rule (one frame for an empty text, `crates/inference-testkit/src/tts.rs:277`). That was my assumption, not a defect; I removed the assertion. It is not a finding.

## 3. Suite run, after the cases existed

Command: `CARGO_INCREMENTAL=0 cargo nextest run -p inference --all-features --no-fail-fast` (the implementor's command plus `--no-fail-fast`, so the count is complete).
```
slot: synced to build-1d in 0.8 s, 126494 bytes sent, 4 sibling mirrors as base, at 54d2130d5 + 2 changed files
    Starting 301 tests across 23 binaries (1 binary skipped via profile.default.default-filter)
     Summary [   3.875s] 301 tests run: 296 passed, 5 failed, 0 skipped
        FAIL [   0.019s] ( 97/301) inference::adversary_w2u1_one_call a_text_agent_tells_the_call_of_its_tool
        FAIL [   3.021s] (297/301) inference::adversary_w2u1_one_call a_chat_whose_tool_panics_returns_from_its_send
        FAIL [   3.028s] (298/301) inference::adversary_w2u1_one_call a_chat_run_closed_with_a_call_held_for_confirmation_ends
        FAIL [   3.021s] (299/301) inference::adversary_w2u1_one_call a_text_agent_requests_the_confirmation_of_the_call_it_holds
        FAIL [   3.522s] (300/301) inference::adversary_w2u1_one_call a_chat_whose_tool_panicked_closes_once_its_send_was_given_up
error: test run failed
exit 100
```
Shortened to these lines; the full 626 lines are in `<scratch>/suite.log`. 301 = 278 + 23 of mine. Every failure is one of my cases; the implementor's 278 pass. Test step 3.9 s, inside the 120 s budget.

## 4. Findings

All rows cover head `54d2130d5` plus my two untracked files. Origin was decided by reading the base with `git show`, never by running it.

| # | file:line | finding | verdict | origin | severity | what reaches it |
|---|---|---|---|---|---|---|
| 1 | `crates/inference/src/recipe/topology.rs:64` | A text agent given a tool that requires confirmation holds the call and never yields `ConfirmationRequested`. `chat` with tools wires the tool loop but exposes no tool event, and `crates/inference/src/agent.rs:320` announces only calls whose `ToolCalled` it saw. The caller never learns the id, so it cannot confirm. | NEEDS-CHANGE | introduced | blocker | `Agent::text().tool(..)`, the path the unit's own doc sends callers to (`crates/inference/src/chat.rs:36`, "Build an `Agent` to confirm calls"). No caller in the repository found. At base `chat` declared no tool loop, so no call could be held. |
| 2 | `crates/inference/src/agent.rs:482` | A text agent's tool call runs with no `Event::ToolCalled`. That event is made only from `SessionEvent::ToolCallProposed`, which the `chat` run never emits. `docs/design/sdk.md:171` says `ToolCalled` precedes the call's confirmation. | CONFIRMED | introduced | warning | `Agent::text().tool(..)`, which the implementor names as a path that now runs calls. No caller in the repository found. Same root as row 1. |
| 3 | `crates/inference/src/chat.rs:236` | `Chat::send` never returns when the tool's executor panics. The wait has three arms (reply part, event, tick) and none fires: the model awaits a result that never comes, and no `Event::Error` arrives before the drain. | CONFIRMED | introduced | warning | `Chat::tool` given a tool whose function panics. No such tool in the repository; a defect in a caller's tool is an ordinary state. The executor's behaviour on a panic (`crates/inference-pipeline/src/nodes/tool_executor.rs:47`) is untouched by the diff; `Chat` is the new path onto it. |
| 4 | `crates/inference/src/chat.rs:298` | `Chat::close` does not return either after that panic. Only dropping the chat closes the session. | CONFIRMED | introduced | warning | The same caller as row 3, after giving the `send` up. I did not check whether `Agent::close` of a `native_voice` agent hangs alike at base. |
| 5 | `crates/inference/src/recipe/run.rs:146` | A `chat` run opened one layer down does not end on `close` while a call is held for confirmation. The wrapper that denies held calls on close is applied to `native_voice` only, though `crates/inference/src/recipe.rs:323` says `chat` "declares the same tool loop". | INFEASIBLE | introduced | warning | Nothing found. `Chat` and `Agent` deny held calls before they close, so only a direct user of `Recipe::chat().tools(..)` and `Instantiation::open` reaches it, and the repository has none. It is a contract drift against the `native_voice` behaviour. |
| 6 | `crates/inference/src/stack.rs:1475` | The diff changes `stack.rs` (`Stack::shared`, bindings now `Arc`) and `recipe.rs`, neither in the story's scope list. `crates/inference-docs/src/status.rs` is in the list and unchanged. | CONFIRMED | introduced | note | The coordinator's check of which units touch which files. No case can test it. |
| 7 | `CHANGELOG.md` | `## [Unreleased]` has no entry for the behaviour changes the unit names (`transcribe` opens; `chat` with tools requires `tool_calling`, so an agent without it no longer starts). | CONFIRMED | introduced | note | `AGENTS.md:45`: a consumer-visible change goes there when it lands on `main`. |

On row 1's severity: before the unit a text agent ignored its tools; now it stalls silently on a held call, and the new doc line points callers at exactly that. If you judge the doc line the only thing to change, it is a warning.

## 5. Attacked and could not break

- A stream dropped half-way, then `send`: the next reply is whole and in order.
- Two calls in one round: the reply waits for both.
- A tool loop of two rounds: the reply holds every turn's text in order.
- The tool loop's bound: `send` errors, the next text starts a new conversation.
- `close`, then `send`: a new conversation, the first session `Closed`.
- `Chat` with a tool that requires confirmation: the call is answered as rejected and the executor does not run.
- `speak_with` on a host without a speaker: errors, and what it opened is closed.
- `talk_with` on a host without a microphone: errors and releases the speaker.
- `Voice` given the empty text: returns, no session left open.
- `Transcriber` given no frame, and `transcribe_with` over a WAV file of no sample: both return, session closed.
- `Transcriber` over a batch recogniser: returns its spans.
- `Transcriber` with 6000 revisions before the commit: spans still returned (no loss on a lagged event stream).
- `Voice::synthesise` of 6000 words, on the multi-thread and on the current-thread runtime: 6000 frames.
- `Send`: every future listed in part 2 is `Send`.
- Acceptance greps, by running `rg`: `Stack::from_env()` matches seven lines, the `assert_send` pattern three.
- Specification first, by reading: `ess/domains/sdk.yaml` declares nothing for chats, agents, events or tools, and the diff adds no noun to it.
- "An agent whose `llm` does not claim `tool_calling` no longer starts", by reading only (I built `-p inference` alone): `rg` found no `Agent::text()` or `Recipe::chat()` given tools in the examples, the adapters, the website docs or the other crates' tests.
- `Replying` across non-chat runs: I wrote no case of my own beyond the chat ones; the existing 278 pass with it in place.

Not attacked: a tool that never answers (it would wait on the executor's 30 s deadline), a synthesiser that fails mid-way, spans of an earlier `Transcriber` call appearing in a later one.

## 6. Paths written outside the worktree

- `<scratch>/run1-cases.log`
- `<scratch>/run2-cases.log`
- `<scratch>/run2-send.log`
- `<scratch>/run3-cases.log`
- `<scratch>/suite.log`

No build directory of my own; builds ran in the shared pool. The scratch directory is deleted 48 h after the session ends.

## 7. Findings block

```findings
- file: crates/inference/src/recipe/topology.rs
  line: 64
  category: contract-drift
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a text agent given a tool that requires confirmation holds the call and never yields ConfirmationRequested, so the call can be neither confirmed nor rejected by id, while chat.rs:36 sends callers to that agent
- file: crates/inference/src/agent.rs
  line: 482
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a text agent runs its tool's call with no Event::ToolCalled, because the chat run with tools exposes no ToolCallProposed
- file: crates/inference/src/chat.rs
  line: 236
  category: boundary
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: Chat::send never returns when the tool's executor panics
- file: crates/inference/src/chat.rs
  line: 298
  category: boundary
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: Chat::close does not return after the tool's executor panicked; only dropping the chat closes its session
- file: crates/inference/src/recipe/run.rs
  line: 146
  category: contract-drift
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: a chat run opened through Instantiation::open does not end on close while a call is held for confirmation, unlike native_voice; no caller in the repository reaches it
- file: crates/inference/src/stack.rs
  line: 1475
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the diff changes stack.rs and recipe.rs, which the story's scope list does not name
- file: CHANGELOG.md
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Unreleased has no entry for the consumer-visible behaviour changes the unit names
```

