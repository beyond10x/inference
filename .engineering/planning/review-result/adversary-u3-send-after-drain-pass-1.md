---
format: aep.planning-md/3
id: review-result:adversary-u3-send-after-drain-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on a send taken before a drain: a cancel during wind-down still answers Ok'
relations:
- reviews: story:live-session-send-after-its-own-turn
revision: 1
---
unit: story:live-session-send-after-its-own-turn, branch impl/live-session-send-after-its-own-turn at b7bbb220 (the findings cover this commit; my test commit on top is 4c7f0346)
verdict: CONFIRMED
cases: executed 117→118, red 1
origin: introduced 0 / pre-existing 1 / undecided 1
wrote-outside-worktree: 6 paths (part 6)
needs-coordinator: whether the Ready-window cancel case is fixed in this unit (the story's Decision promises it) or routed to backlog as pre-existing

| file:line | what is wrong | verdict | origin |
|---|---|---|---|
| crates/inference-facade/src/live.rs:1033 | A cancel that overtakes a taken send while the cancelled run is still winding down gets `Ok`, not `Refused`. It also records a second `InputSent` and pushes the text into the turn. The `!self.cancelled` guard only runs on a `WrongState` error, and the session is still Ready at that point. This breaks the doc at `:1004-1005` and the Decision's "a session cancelled while the ingress took the envelope still answers Refused". The header and test name in `send_taken_then_drained.rs` ("never answered Ok") are wrong for the same reason. | CONFIRMED | pre-existing |
| crates/inference-facade/src/live.rs:1039 | Mutant that survives: deleting `SessionState::Closed` from the `matches!` keeps the suite green. Only the Draining arm is tested. A Closed-by-drain state when the send is recorded happens only if the thread is preempted between the push and the `sessions` lock, so I could not build a test that always reaches it. | INFEASIBLE | undecided |

**1. `git --no-pager diff --stat`** (after my commit; the working tree has no changes). Range `b7bbb220..4c7f0346`:
```
 crates/inference-facade/tests/adversary_u3_send_after_drain.rs | 142 +++++
 1 file changed, 142 insertions(+)
```
Every changed path is a test file. I changed no implementation file.

**2. Case added** in `crates/inference-facade/tests/adversary_u3_send_after_drain.rs`: `a_send_the_ingress_took_while_the_cancelled_run_winds_down_answers_refused`. It is red now.
- **Setup:** ingress capacity 1 with Pause. "first" is buffered and "second" waits. Then `cancel()`. The node's `cancel()` takes one envelope, which frees room so the waiting send pushes its own. The node then holds its cancel open until the send has answered, so the run has not ended.
- **Asserts:** the session is still Ready (this passes), the send answers `Refused`, one `InputSent` in total, and `inputs_sent == 1`.

First run of the case alone (`cargo test -p inference-facade --all-features --test adversary_u3_send_after_drain`), before the suite:
```
test a_send_the_ingress_took_while_the_cancelled_run_winds_down_answers_refused ... FAILED
thread '...' panicked at crates/inference-facade/tests/adversary_u3_send_after_drain.rs:133:5:
a session cancelled while the ingress took the envelope answers Refused: Ok(())
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s
exit 101
```
- **On the base:** the same case run against a `git archive ff0ba010` copy in scratch fails the same way (`:133:5 ... answers Refused: Ok(())`), so it is pre-existing.
- **Who reaches it:** any application that calls `cancel()` while a send is waiting on a full ingress, if a node still reads one more input before its cancel completes. `wind_down` (`inference-runtime/src/controller.rs:1159-1172`) runs `node.cancel()` before the run ends, and the session stays Ready until `CancelSession`.
- **Fix to name (not applied):** after the forward, answer `Refused` whenever `self.cancelled` is set, in any state, before recording `InputSent`.

**3. Suite run** after the case existed: `nice cargo test -j 8 -p inference-facade --all-features --no-fail-fast`. 117 passed and 1 failed, which is my case. Exit 101.
```
error: 1 target failed:
    `-p inference-facade --test adversary_u3_send_after_drain`
```
The earlier run without `--no-fail-fast` stopped at my binary, so I re-ran it with the flag. The "117" before-count is this same run with my one case left out; it is not a separate run.

**4. Judgement findings:** the Closed-arm mutant in row 2. Nothing reaches it except preemption under load, where the agent load test might catch it some of the time. This covers b7bbb220.

**5. Attacked and not broken:**
- **Flag ordering:** `cancelled` is set (`:1181`) before `shared.cancel()` can move the state to Closed, so a cancel always reads as a cancel once the state has moved.
- **Cancel after a completed drain:** this cannot turn a taken send into `Refused`. `cancel()` on a Closed session returns early without setting the flag, and the send's push and its record happen in one poll.
- **Send refused before the take:** this cannot now answer `Ok`. The `admits_input` check comes before the forward, and a send to a closed ingress still answers `Forward`.
- **`InputSent`, usage and turn texts for a send taken while Draining:** no event, not counted, no text pushed. The implementor's test asserts this.
- **The no-cancel-guard mutant:** the existing cancel test rejects `Ok`, so it catches it.
- **Clippy:** `cargo clippy -p inference-facade --all-features --tests -D warnings` is clean.

**6. Paths written outside the worktree** (all in the unit's scratch directory `adv1/`): `case.log`, `suite.log`, `suite-nff.log`, `clippy.log`, `commit-msg.txt`, and `base/`, the `git archive` copy of ff0ba010 with its own `target/` (274M), plus `base-case.log` written next to it.

My lease `inference-u3-adv1` is released.

```findings
- file: crates/inference-facade/src/live.rs
  line: 1033
  category: acceptance
  severity: blocker
  verdict: CONFIRMED
  origin: pre-existing
  message: a cancel that overtakes a taken send while the cancelled run winds down leaves the session Ready, so send answers Ok and records a second InputSent instead of the Refused the doc and the story's Decision promise
- file: crates/inference-facade/src/live.rs
  line: 1039
  category: mutant
  severity: note
  verdict: INFEASIBLE
  origin: undecided
  message: removing SessionState::Closed from the drain guard survives the suite because Closed-by-drain at record time is reachable only by preemption between the push and the sessions lock
```
