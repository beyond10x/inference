---
format: aep.planning-md/3
id: review-result:adversary-u2-cascade-burst-pass-2
kind: review-result
status: active
title: 'Adversary pass 2 on cascade turns in a burst: three surviving mutants closed, a dead guard, the held lane end undocumented'
relations:
- reviews: story:cascade-turn-spans-in-a-burst
revision: 1
---
```
unit: story:cascade-turn-spans-in-a-burst, branch impl/cascade-turn-spans-in-a-burst; findings cover e156ea09; cases committed as 34676768
verdict: CONFIRMED (three surviving mutants of the correction, now closed by green cases); no red case in the suite
cases: executed 274→283, red 0
origin: introduced 5 / pre-existing 0 / undecided 1
wrote-outside-worktree: 34 files plus the adv2/ directory (part 6)
needs-coordinator: mutation ran inside the worktree as allowed, which the adversary procedure forbids; each file was restored right away and the tree is clean.
```

1. **`git --no-pager diff --stat e156ea09..HEAD`.** Both paths are test files, and the tree is clean.
```
 .../tests/adversary_u2_pass2_gate_ends.rs          | 356 +++
 .../tests/adversary_u2_pass2_reattribution.rs      | 319 +++
 2 files changed, 675 insertions(+)
```

2. **Cases.** All are green now. Each one targets a mutant that nothing else catches (part 3).
   - `crates/inference-pipeline/tests/adversary_u2_pass2_gate_ends.rs`
     - `turn_events_read_after_the_text_lanes_end_was_taken_reach_the_text_lane_ahead_of_it`: the gate takes the text lane's `stop` while it drains for c1, and o2 and c2 come later. Asserts the text lane gets `[span, o1, c1, o2, c2, STOP]` and `turns_passed == [t1, t2]`.
     - `a_gate_of_one_lane_passes_each_turn_event_once`.
   - `crates/inference-testkit/tests/adversary_u2_pass2_reattribution.rs`
     - `spans_of_two_turns_never_completed_are_answered_by_the_third_turns_generation`: three turns open with none completed. Asserts one generation over `[one, two, three]`.
     - `a_span_of_a_later_turn_held_at_a_reopening_stays_with_its_own_turn`.
   - **Probe, not committed** (`adversary_u2_pass2_probe_hold.rs` in scratch). The text lane gets `[span, STOP]` and the turn lane gets `[o, c]` and never ends. Red on its first run:
     ```
     assertion `left == right` failed: the text lane's end of input is passed once its spans and the turn's events were
       left: [Content(SpanCommitted {..}), Content(Turn(Opened(..))), Content(Turn(Completed(..)))]
      right: [Content(SpanCommitted {..}), Content(Turn(Opened(..))), Content(Turn(Completed(..))), Control(Stop)]
     ```
   - **Agent-count cases, all green, not committed** (`adversary_u2_pass2_counts.rs` in scratch): burst then immediate close 16×15; burst 1×30 and 48×5; turn completed by the close 1×30 and 48×5.

3. **Mutants and suites.**
   - **Mutants** (in the worktree, each file restored straight after; diffs in scratch):
     - **Md**: copy filter `lane != 0` changed to `true`. The pipeline suite missed it; only the one-lane case fails.
     - **Me**: drop `self.inputs[1..].iter().all(Option::is_none) &&` at `commit_gate.rs:597`. The pipeline suite missed it, and so did `adversary_u2_turn_at_close` and `cascade_burst`. Only the drain case fails.
     - **Mg**: `*cause == Some(old)` changed to `cause.is_some()` at `agent.rs:519`. The pipeline suite and the four existing testkit cases missed it; only the later-turn case fails (`left: [t2]`, `right: [t2, t3]`).
     - **Mi**: `if self.text_closed` changed to `if false` at `commit_gate.rs:636`. Everything stays green, because the mutant is equivalent (finding 4).
   - **Suites**, after the cases existed, all exit 0:
     - `cargo test -p inference-pipeline --all-features`: 157 passed
     - testkit (`agent_turn_order`, `agent_keeping_replies`, `adversary_u2_agent_reopened_turn`, `adversary_u2_stt_names_turn` and the new file): 8 passed
     - `cargo test -p inference --features testing --test cascade_burst --test recipes --test agent --test adversary_u2_turn_at_close --test adversary_u2_pass2_counts`: 106 passed (cascade_burst 1, recipes 35, agent 64, turn_at_close 1, counts 5)
     - `cargo test -p local-voice-cascade --tests`: 12 passed
   - "Before" is the same runs minus the 9 new cases (2 pipeline, 2 testkit, 5 inference).
   - Clippy with `-D warnings` and rustfmt are clean on both committed files.

4. **Findings** (covering e156ea09):

| # | file:line | what | verdict | origin |
|---|---|---|---|---|
| 1 | `crates/inference-pipeline/src/nodes/commit_gate.rs:597` | Me survived every gate and cascade test: taking the held end after any envelope ends the text lane before later turn events, and their copies are dropped. Closed by the drain case. | CONFIRMED | introduced |
| 2 | `crates/inference-pipeline/src/nodes/commit_gate.rs:587` | Md survived: copying turn events taken on lane 0 makes a one-lane gate pass each one twice. Closed. In the cascade, lane 0 never carries turn events. | CONFIRMED | introduced |
| 3 | `crates/inference-pipeline/src/nodes/agent.rs:519` | Mg survived: re-attributing every held span that names any opening moves a later turn's span into the reopened turn, and the later turn gets no generation. Closed. Only reachable through a turn store that refuses a completion. | CONFIRMED | introduced |
| 4 | `crates/inference-pipeline/src/nodes/commit_gate.rs:636` (field at `:507`) | `text_closed` is dead. Lane 0's end is only taken once every other lane is done, so `pass_copy` never runs after it, and Mi is equivalent. The doc at `:634`, "if that lane still goes", no longer describes anything. Fix: remove the field and the guard. | CONFIRMED | introduced |
| 5 | `crates/inference-pipeline/src/nodes/commit_gate.rs:55` | The module doc says the end of input on a lane "passes the rest and then the `stop`". The first lane's `stop` actually waits until every other lane ends, and forever if one never does (probe red). Only `take`'s doc and the field doc say so. In `voice_cascade` and the example, the STT ends `turns_out` before `transcript`, so nothing found reaches the indefinite hold. Fix: one line in the module doc. | INFEASIBLE | introduced |
| 6 | `crates/inference-pipeline/src/nodes/agent.rs:536` | `turn_completed` ignores the completion's turn id. A late `TurnCompleted` for an old opening after a reopen answers the new turn early, and the new turn's later spans are dropped. The turn policy never sends a refused completion. Not run on the base, though the base reads the same way. | INFEASIBLE | undecided |

5. **Attacked and could not break:**
   - **Deadlock from backpressure on the turn lane:** the speaker sink always reads turns ahead, so it never stops taking them.
   - **Cancelled or dropped upstream:** the edges close, the gate gets `None` on both lanes, and it ends.
   - **Runtime drain:** it drains upstream first and closes no edges.
   - **A copy made twice or out of order under drain or close:** each lane-1 envelope is taken once.
   - **`turns_passed`:** it stayed exact in every case.
   - **Re-attribution through three uncompleted turns:** it holds.
   - **A span naming the old opening after a reopen:** unreachable. The STT flushes the old recognition before it sends the new `TurnOpened`, and the gate drains lane 0 first.
   - **Agent counts:** 1, 16 and 48 agents, and a burst closed at once, are all green.
   - **The example's gate tests:** 12 of 12 pass.

6. **Paths written outside the worktree**: 34 files plus the directory, all in the unit's `adv2/` scratch directory (logs, status files, mutant diffs, scripts, harness fragments, two uncommitted case files, the commit message).

7. Findings block:
```findings
[
  {"file": "crates/inference-pipeline/src/nodes/commit_gate.rs", "line": 597, "category": "mutant", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "taking the held text-lane end after any envelope instead of once every other lane ended survived every gate and cascade test; closed by the drain case in adversary_u2_pass2_gate_ends.rs"},
  {"file": "crates/inference-pipeline/src/nodes/commit_gate.rs", "line": 587, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "dropping lane != 0 from the copy filter, which doubles every turn event of a one-lane gate, survived the pipeline suite; closed by the one-lane case"},
  {"file": "crates/inference-pipeline/src/nodes/agent.rs", "line": 519, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "re-attributing every held span that names any opening survived the agent tests and moves a later turn's span into the reopened turn; closed by adversary_u2_pass2_reattribution.rs"},
  {"file": "crates/inference-pipeline/src/nodes/commit_gate.rs", "line": 636, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "text_closed is dead: lane 0's end is taken only after every other lane ended, so the pass_copy guard never fires and its doc 'if that lane still goes' no longer describes anything"},
  {"file": "crates/inference-pipeline/src/nodes/commit_gate.rs", "line": 55, "category": "contract-drift", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "the module doc says a lane's end of input passes its stop, but the first lane's stop is held until every other lane ends and forever if one never does (probe red); no repository topology was shown to reach the indefinite hold"},
  {"file": "crates/inference-pipeline/src/nodes/agent.rs", "line": 536, "category": "judgement", "severity": "note", "verdict": "INFEASIBLE", "origin": "undecided", "message": "turn_completed ignores the completion's turn id, so a late TurnCompleted of an old opening after a reopen answers the new turn early and its later spans are dropped; the turn policy never sends a refused completion"}
]
```
