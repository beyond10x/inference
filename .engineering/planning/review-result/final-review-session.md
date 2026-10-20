---
format: aep.planning-md/3
id: review-result:final-review-session
kind: review-result
status: active
title: Final adversary review, session area
relations:
- reviews: story:facade-crate
- reviews: story:session-live-text
- reviews: story:session-history
- reviews: story:session-interrupt-and-cancel
- reviews: story:session-topology-switch
revision: 1
---
unit: session area (`ess/domains/session.yaml`, `crates/inference-facade`, testkit session store and target) in the working tree at c733f12, diff 8c4da2c..c733f12
verdict: NEEDS-CHANGE
cases: executed 32→40 (inference-facade), red 6
origin: introduced 7 / pre-existing 0 / undecided 0
wrote-outside-worktree: 10 paths, all under <host path>
needs-coordinator: none

**1. Diff stat.** `git --no-pager diff --stat` prints nothing: no tracked file changed. There are 4 untracked files, all of them tests:
`crates/inference-facade/tests/adversary1_{interrupt_model_node,switch_drain,handle_and_history,delivery_guards}.rs`

**2. Cases added.** Each red case was run alone before the suite.

| case | asserts | now | red output when written |
|---|---|---|---|
| interrupt_model_node::after_an_interrupt_the_cut_generations_committed_segment_is_not_delivered_nor_recorded | text pipeline with the testkit `ModelNode`; after the first TextDelta, `interrupt` → no TextSegmentCommitted is delivered and no assistant entry is recorded | red | `the interrupted generation's segment is delivered after the interrupt: [TextSegmentCommitted { .., text: "s1" }]` |
| switch_drain::the_old_runs_egress_tail_is_delivered_before_its_run_completed | switch is polled once, then the old driver runs to its end (the order a single-thread join uses) → GenerationCompleted(old) comes before RunCompleted(old) | red | `the old run's egress tail arrives after its RunCompleted: [..PipelineSwitched, RunCompleted, GenerationStarted, GenerationCompleted, RunAttached]` |
| switch_drain::a_session_whose_switch_did_not_drain_is_closed_by_cancel | old run fails in drain → `Drained(Failed)`; then `cancel()` → session Closed | red | `cancel left a session with no run unclosed  left: Some(Opened) right: Some(Closed)` |
| switch_drain::a_session_whose_switch_never_drains_is_closed_by_cancel_once_the_switch_is_dropped | drain never finishes, switch future dropped, `cancel()` → old run Cancelled and session Closed | red | `cancel left the session of an abandoned switch unclosed  left: Some(Opened) right: Some(Closed)` |
| handle_and_history::a_ready_session_whose_handle_is_dropped_closes_once_its_run_has_ended | handle dropped while Ready → run ends within 5 s and session is Closed | red | `after the handle was dropped: run Err(Timeout), session Some(Ready)` |
| handle_and_history::the_user_turn_precedes_the_reply_that_answers_it_when_the_reply_is_received_before_the_commit | send "1", receive the segment, then commit → history is [user "1", assistant "s1"] | red | `left: [(1, AssistantSegment, "s1"), (2, UserTurn, "1")]` |
| delivery_guards::after_cancel_nothing_the_egress_still_holds_is_delivered | `cancel` delivers none of the deltas already queued | green guard | kills mutant live.rs:791 |
| delivery_guards::an_interrupt_cuts_a_generation_only_its_events_name | envelopes carry `generation_id: None`; after the interrupt no TextDelta of the cut generation is delivered | green guard | kills mutant live.rs:848 |

**3. Suite run, after the cases existed.**
- `cargo test -p inference-facade --all-features --no-fail-fast` → EXIT=101.
  - history 3/3, interrupt_and_cancel 3/3, session 19/19, text_session 5/5, topology_switch 2/2, delivery_guards 2/2.
  - handle_and_history 0/2, interrupt_model_node 0/1, switch_drain 0/3.
- clippy `-D warnings` EXIT=0. `cargo fmt --check` EXIT=0.

Mutants, run in a scratch copy that was cleaned and removed afterwards (log: `mutants.log`):

| mutant | result |
|---|---|
| switch.rs:146, drop the old run's tail | killed by topology_switch |
| live.rs:848, no `generation_of` fallback | survives the unit's suite; my guard kills it |
| live.rs:791, `cancelled` never set | survives the unit's suite; my guard kills it |

**4. Findings** (tree c733f12 plus the untracked tests)

| # | file:line | verdict | origin | what was measured / what reaches it |
|---|---|---|---|---|
| F1 | crates/inference-facade/src/live.rs:848 | NEEDS-CHANGE | introduced | **Measured:** red case above. A segment of the cut generation is still delivered after the interrupt and recorded as an assistant entry. **Cause:** a TextSegmentCommitted names no generation, and `generation_of` (:936) leaves it out. **Reaches it:** the testkit ModelNode, which every facade test runs, sends segments with `generation_id: None` (`graph/nodes.rs:335`, `stream.rs:292`). The acceptance test passes only because its own `Streamer` puts the generation on every envelope. ModelNode also runs one generation per run, so after one interrupt every later delta of that run is cut while segments keep arriving. |
| F2 | crates/inference-facade/src/live.rs:186 | NEEDS-CHANGE | introduced | **Measured:** `run_ended` for a run the session left puts RunCompleted into `events` straight away. `poll_drained` (switch.rs:449) then queues the old egress tail behind it. **Reaches it:** any executor that polls the driver to its end between two polls of `switch`. That is a single-thread join, and a race with a driver thread. |
| F3 | crates/inference-facade/src/live.rs:228 | NEEDS-CHANGE | introduced | **Measured:** two red cases. `Shared::cancel` only finishes the close when `inbox.ended` is set. After `leave()` there is no current run, so a left run never sets it. After `Drained`, or after a dropped switch future, `cancel` leaves the session Opened, `close` leaves it Draining, and `receive` stays Pending. Nothing can close it. **Reaches it:** `SwitchError::Drained` is a documented return (a node failing in drain), and an app that bounds the switch with a timeout drops the future. |
| F4 | crates/inference-facade/src/live.rs:911 | NEEDS-CHANGE | introduced | **Measured:** red case. `Drop` only calls `release()`. Dropping the ingresses does not end the run's input, so the run stays Active, `drive()` never returns, and the session stays Ready. This contradicts the module documentation at :27-30. **Reaches it:** any app that drops the handle without `close` (an early return, a panic). |
| F5 | crates/inference-facade/src/live.rs:697 | CONFIRMED | introduced | **Measured:** red case. The user turn is recorded when the app commits, but ModelNode answers each TranscriptCommitted as soon as it arrives. A reply received before the commit lands at position 1 and the turn it answers at position 2. **Reaches it:** the text pipeline with an app that reads before it commits. history.rs only passes because it commits right after sending. |
| F6 | crates/inference-facade/src/live.rs:848 | CONFIRMED | introduced | **Measured:** the mutant that drops the `generation_of` fallback survives the unit's suite, because every test node puts the generation on the envelope. **Reaches it:** test gap only. My guard closes it. |
| F7 | crates/inference-facade/src/live.rs:791 | CONFIRMED | introduced | **Measured:** the mutant that never sets `cancelled` survives. The cancel tests check only the last event and RunCancelled, never that nothing more was delivered. **Reaches it:** test gap only. My guard closes it. |

Fixes I would name but did not apply:
- **F1:** map each segment to its generation from the TextDelta (segment_id → generation_id) in `deliver`, or have `Emitter::stream` put the generation on the segment envelope.
- **F2:** keep the left run's ending until its egresses have ended.
- **F3:** have the left run's end set `ended`, or close at once in `cancel` when no run is current.
- **F4:** in `Drop`, send `stop` or call `requests.cancel()`.

**5. Attacked and could not break**
- SessionClosed is delivered last and only once on the drain, cancel and release paths.
- `send` with an unknown port while Ready gives UnknownIngress and records no InputSent. Input outside Ready is refused.
- Several sends before one commit are joined into one user turn.
- Nothing is recorded after Closed.
- An unknown pipeline or a mode mismatch at switch is refused before anything moves. Input during a switch is impossible because `switch` takes `&mut self`.
- The state machine refuses in the order the ESS spec gives. The forced SwitchPipeline externals are answered without the store, as ESS allows for external outcomes.
- The switch.rs:146 tail mutant is killed by the existing suite.

**6. Paths written outside the worktree**
- In `<host path>`: `build1.log`, `clippy.log`, `fmt.log`, `guards.log`, `handle.log`, `mutants.log`, `mutate.sh`, `suite.log`, `switch.log`, `switch2.log`.
- The `mutant/` copy was cleaned with `cargo clean` and removed.

**7. Findings block**
```findings
- {"file": "crates/inference-facade/src/live.rs", "line": 848, "category": "acceptance", "severity": "blocker", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "After an interrupt, the cut generation's TextSegmentCommitted is still delivered and recorded in history, because neither its envelope (testkit ModelNode) nor generation_of names its generation."}
- {"file": "crates/inference-facade/src/live.rs", "line": 186, "category": "concurrency", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "At a switch, the left run's RunCompleted is queued as soon as its driver ends, so egress events the switch drains afterwards are delivered after the run's end."}
- {"file": "crates/inference-facade/src/live.rs", "line": 228, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "After SwitchError::Drained or a dropped switch future, cancel and close never reach Closed, because only the current run's end sets inbox.ended."}
- {"file": "crates/inference-facade/src/live.rs", "line": 911, "category": "contract-drift", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "Dropping a Ready handle leaves the run Active and its driver pending forever, and the session stays Ready, contrary to the module documentation."}
- {"file": "crates/inference-facade/src/live.rs", "line": 697, "category": "acceptance", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "The user turn is recorded at commit, so a reply received before the commit precedes the turn it answers in history."}
- {"file": "crates/inference-facade/src/live.rs", "line": 848, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "Removing the generation_of fallback survives the facade suite; adversary1_delivery_guards kills it."}
- {"file": "crates/inference-facade/src/live.rs", "line": 791, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "Never setting the cancelled flag survives the facade suite; adversary1_delivery_guards kills it."}
```