---
format: aep.planning-md/3
id: review-result:adversary-u3-send-after-drain-pass-2
kind: review-result
status: active
title: 'Adversary pass 2 on a send taken before a drain: a send after a cancel is forwarded, a made-up Closed state'
relations:
- reviews: story:live-session-send-after-its-own-turn
revision: 1
---
```
unit: story:live-session-send-after-its-own-turn, at 65910e3b (cases committed on top as 52e98eb8)
verdict: NEEDS-CHANGE
cases: executed 118→121, red 2
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 9 paths under adv2/ (listed in part 6)
needs-coordinator: whether a cancelled send should carry the real state (Ready) or get its own error variant, which changes the InputError type
```

**1. Diff stat** (`git --no-pager diff --stat 65910e3b..HEAD`; the tree is clean, 0 uncommitted paths)
```
 .../tests/adversary_u3_send_after_drain_pass2.rs   | 281 +++++++++++++++++++++
 1 file changed, 281 insertions(+)
```
It is a test file only. Charter deviation: you told me to test mutants in the worktree, and the adversary procedure forbids that. I followed your instruction once, for mutant M1 (part 2, case 3). The script changed `live.rs:1037` with sed, built the test binaries with `--no-run`, and ran `git checkout -- crates/inference-facade/src/live.rs` straight away, before running any binary. `git status` afterwards showed only my untracked test file. The file was mutated only for the length of that build.

**2. Cases added** to `crates/inference-facade/tests/adversary_u3_send_after_drain_pass2.rs`. All three use one node whose cancel takes N envelopes from its input and then holds, so the cancelled run is still winding down when the send answers. The two red cases were red on 20 of 20 repeated runs.

- `a_send_started_after_the_cancel_that_answers_refused_forwards_nothing` (**red**). Flow: `cancel()`, then the node takes the first envelope, then a fresh `send("late")` runs while the session is still Ready. That send passes `admits_input`, is forwarded, and then answers `Refused`. The cancelled run's node found the envelope in its input. The `InputError::Refused` doc at `live.rs:630` says "Nothing was forwarded". Red output, run alone:
  ```
  panicked at crates/inference-facade/tests/adversary_u3_send_after_drain_pass2.rs:184:5:
  InputError::Refused promises nothing was forwarded (live.rs:630), yet the late send's envelope reached the cancelled run's node
  ```
- `a_refused_send_the_cancel_overtook_names_the_state_the_session_holds` (**red**). This is pass 1's scenario, now checking the error's carried state. `live.rs:1040-1045` turns `admits_input(..).err()` = None into a made-up `Closed`. Red output, run alone:
  ```
  panicked at crates/inference-facade/tests/adversary_u3_send_after_drain_pass2.rs:220:5:
  assertion `left == right` failed: SessionStateConflict.current is the state the session holds (session.rs:300-301); the session held Ready when the send answered
    left: WrongState(SessionStateConflict { current: Closed })
   right: WrongState(SessionStateConflict { current: Ready })
  ```
- `a_send_taken_while_draining_that_a_cancel_overtook_answers_refused` (**green**; it catches a mutant nothing else catches). Flow: a send is waiting, `close()` moves the session to Draining, `cancel()` follows, and the node takes the first envelope, the second and the stop. The send answers `Refused(WrongState{Draining})` and is not counted.
  - Mutant M1: `if self.cancelled.load(..) && sessions.admits_input(id).is_ok()`, which skips the cancel check for a session that is not Ready.
  - Under M1, 30 of the package's 31 test binaries passed, including `send_taken_then_drained` and pass 1's `adversary_u3_send_after_drain`. Only this file failed: `...pass2.rs:265:5: ... answers Refused, not the Ok of a drain: Ok(())`.

After `rustfmt`, the red lines are now :185 and :222. The rerun gave the same messages.

**3. Suite run** (after the cases existed): `nice cargo test -j 8 -p inference-facade --all-features --no-fail-fast`, exit 101.
```
test a_send_started_after_the_cancel_that_answers_refused_forwards_nothing ... FAILED
test a_refused_send_the_cancel_overtook_names_the_state_the_session_holds ... FAILED
test result: FAILED. 1 passed; 2 failed; ...
error: 1 target failed:
    `-p inference-facade --test adversary_u3_send_after_drain_pass2`
```
Across the run's binaries: 119 passed, 2 failed. The "before" count of 118 is the same run with my file's 3 cases taken out. `cargo clippy -p inference-facade --all-targets --all-features -- -D warnings` is clean.

**4. Findings** (they cover 65910e3b)

| file:line | what | verdict | origin |
|---|---|---|---|
| crates/inference-facade/src/live.rs:1019 | `send` checks only `admits_input` before forwarding. So a send started after `cancel()`, while the session is still Ready, forwards its envelope into the cancelled run and then answers `Refused`, which contradicts `live.rs:630`. The base answered `Ok` there. Fix: check `self.cancelled` next to `admits_input`, before the forward. Reached by any facade user whose send runs after `cancel()` returned, while the run is still winding down. | NEEDS-CHANGE | introduced |
| crates/inference-facade/src/live.rs:1040 | A send overtaken by a cancel while the session is Ready carries `WrongState{current: Closed}`, a state the session does not hold. That contradicts `session.rs:300-301`, and `agent.rs:1102` passes it on as the text "the session is Closed". Reached in pass 1's scenario. The fix needs your decision: either carry the real state, or add a cancelled variant, which changes `InputError`. | CONFIRMED | introduced |

**5. Attacked and could not break**
- A cancel racing a drain: answers Refused with Draining and is not counted (case 3).
- Usage counts: `inputs_sent` does not count an overtaken send in any of the three cases.
- A send whose envelope the cancelled run processes, as opposed to one it discards: I could not build this deterministically. The queue push and the session record happen in a single poll (`queue.rs` `poll_send`), so only another thread can get in between.
- The `Closed` part of the `Draining | Closed` arm (`live.rs:1050-1052`): dropping `Closed` is a mutant no deterministic case can reach. Once the drain has ended the run, the waiting push gets `Forward`, so only that same in-between race reaches the arm.
- The race between `cancel()`'s state check and its flag write (`live.rs:1181-1195`) against a drain finishing: there is no way to make it deterministic.

**6. Paths written outside the worktree**, all in the unit's scratch directory `adv2/`: `case.log`, `mutant.sh`, `mutant.line`, `mutant-build.log`, `mutant-status.txt`, `mutant-bins.txt`, `mutant-run.log`, `suite.log`, `commit-msg.txt`. My lease `inference-u3-adv2` is released.

**7. Findings block**
```findings
[
  {"file": "crates/inference-facade/src/live.rs", "line": 1019, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "A send started after cancel() while the session is still Ready is forwarded into the cancelled run and then answered Refused, though InputError::Refused promises nothing was forwarded; checking the cancelled flag before the forward fixes it."},
  {"file": "crates/inference-facade/src/live.rs", "line": 1040, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "A Refused for a send overtaken by a cancel while the session is Ready carries a made-up SessionStateConflict { current: Closed }, a state the session does not hold."}
]
```
