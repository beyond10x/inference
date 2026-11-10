---
format: aep.planning-md/3
id: story:live-session-send-after-its-own-turn
kind: story
status: active
title: A session's send of an input the run took is never refused by the close that input caused
tags:
- from-finding
relations:
- serves: vision:inference
- decomposes: epic:defects-2026-11-10
scope:
- confidence: cited
  path: crates/inference-facade/src/live.rs
- confidence: inferred
  path: crates/inference-facade/tests/send_taken_then_drained.rs
- confidence: cited
  path: crates/inference/tests/agent.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:16:58Z", actor: "human:operator", revision: 6}
- {from: "proposed", to: "active", at: "2026-11-10T00:16:58Z", actor: "human:operator", revision: 7}
---
## Context

Found on 2026-11-08 while the fourth SDK wave made its agent cases deterministic under load; it
lives in `inference-facade` and predates that wave.

`LiveSession::send` (`crates/inference-facade/src/live.rs:1023-1030`) hands the envelope to the
run first and records the send afterwards. When that envelope completes a turn whose reply makes
the model close the session (the duplex mock `.closes()` once its script is spent), the session
can already be Draining when the send is recorded, and the send answers `InputError::Refused`
("the session is Draining") for an input the run took. Its documentation allows a refusal only
before the input was taken. Seen in about 6 of 1,000 lanes of
`cargo test -p inference --features testing --test agent` with six lanes at once (load 14-33),
each in a different test, through the test helper `speak()`; the mock had taken every frame.

## Acceptance

- An input the run took is answered `Ok`, whatever the session's state when the send is recorded;
  a send refused before the run took it answers `Refused` with its envelope, as now.
- A test that closes the session from the model at the input that completes the turn sees `Ok`
  for that send in every one of 1,000 rounds at 16 sessions at once.

## Scope

Derived 2026-11-10 by `story-scoper` against `main` at `af93222a`. Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/inference-facade` — cited (story Context names `inference-facade`)
- **Files:** `crates/inference-facade/src/live.rs:1006-1035` (`LiveSession::send`, doc at `:995-1005`) — cited (the story names `live.rs:1023-1030`; on main the forward is at `:1023-1027` and the record at `:1028-1030`)
- **Symbols:** `LiveSession::send`, `Sessions::send_input` (`crates/inference-facade/src/session.rs:686`), `InputError::Refused` (`live.rs:629-631`) — cited
- **Test (red/green, deterministic):** a new file `crates/inference-facade/tests/send_taken_then_drained.rs`, run with `cargo test -p inference-facade --test send_taken_then_drained`. It needs a node that holds the ingress (capacity 1, `OverloadPolicy::Pause`, as `tests/shared_session.rs` does) until `close()` has moved the session to Draining — inferred
- **Test (acceptance load case):** `crates/inference/tests/agent.rs` (helper `speak()` at `:92`, `.closes()` mocks at `:135`, `:167`, `:237`, `:281`, `:303`), run with `cargo test -p inference --features testing --test agent` — cited (story Context; `crates/inference/Cargo.toml:71-73`)
- **Also likely:** `CHANGELOG.md` `## [Unreleased]`, one line, because the change is visible to consumers — inferred (rule at `AGENTS.md:41`)
- **Also possible, only if the fix records `InputSent` while Draining:** `crates/inference-facade/src/session.rs:686` and `ess/domains/session.yaml:496-516` (`SendInput` admits Ready only). The minimum fix records nothing and needs neither — inferred
- **Documents:** none beyond the `CHANGELOG.md` line — inferred
- **Confidence:** high. The story names the defect site, and reading confirms it on main
- **Would collide with:** any unit editing `crates/inference-facade/src/live.rs`. That file is the facade's shared hot spot (1,864 lines: send, commit, close, interrupt, receive, usage, pricing, its last 5 commits are all facade fixes or features). Also any unit adding a line under `CHANGELOG.md` `## [Unreleased]` — cited (file size and `git log` read)
- **Safety fact:** `admits_input` (`live.rs:1012-1014`) still refuses a session that is not Ready before anything is forwarded. So turning a post-forward `WrongState { current: Draining }` into `Ok` (`live.rs:1028-1030`) changes only sends the ingress already took. `Closed` (cancel, `session.rs:842-857`) stays `Refused`, as the doc at `live.rs:1004-1005` requires — step 2, unproven

What the scoper could not establish:
- The acceptance's "Refused with its envelope, as now" does not match the code. `InputError::Refused(SessionMoveError)` (`live.rs:631`) carries no envelope, so either the story is wrong or the fix also changes `InputError`.
- "Whatever the session's state" conflicts with the documented cancel case (`live.rs:1004-1005`). Both cancel and a completed drain end in `Closed` (`session.rs:842-857`, `CompleteSessionClose`), so a send that is overtaken by a full drain-and-close cannot be told apart from a cancelled one by state alone.
- Whether `Agent::send_audio` (`crates/inference/src/agent.rs:1284-1289`) and `input_error` (`:1100-1110`) need any change once the facade returns `Ok`; reading says they don't.

Defect confirmation on `main` at `af93222a` (by reading):
1. `crates/inference-facade/src/live.rs:1023-1027` forwards the envelope to the ingress first, then `:1028-1030` runs `send_input`, which `?`-maps a non-Ready state to `InputError::Refused`. `send_input` accepts Ready only (`crates/inference-facade/src/session.rs:686-687`).
2. The mock's `.closes()` emits `SpeechSessionClosed`. The agent sets `Close::Drain` (`crates/inference/src/agent.rs:444-447`), its task calls `drain()` → `session.close()` (`agent.rs:746`, `:711-714`), and that moves the session to Draining (`live.rs:1074-1077`, `session.rs:661-672`) between the forward and the record.
3. The result is `Refused` ("the session is Draining") for an input the run took, contradicting the doc's "nothing is forwarded" for `Refused` (`live.rs:1000-1001`, `:630`). `agent.rs:1102-1104` passes it on to `speak()` as `Error::Closed`.

## Decision

Taken by the coordinator on 2026-11-10, before code, from the scoper's two findings:

- "Refused with its envelope, as now" is read as "Refused, as now": `InputError::Refused` carries a
  `SessionMoveError` and no envelope (`crates/inference-facade/src/live.rs:629-631`), and this story
  does not change that type.
- "Whatever the session's state" means: once the ingress took the envelope, `send` answers `Ok`
  when the session moved to Draining, or on to Closed by that drain, while the send was in flight.
  The documented cancel case stays: a session cancelled while the ingress took the envelope still
  answers `Refused` (`live.rs:1004-1005`). Whether a later state was reached by a drain or by a
  cancel is told from what the session recorded, not from the state alone.
- `InputSent` is recorded only where `SendInput` admits it (Ready); an input taken after the session
  left Ready answers `Ok` without an `InputSent`, and the doc comment of `send` says so.
