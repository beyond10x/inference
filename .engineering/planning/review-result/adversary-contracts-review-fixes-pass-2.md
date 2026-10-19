---
format: aep.planning-md/3
id: review-result:adversary-contracts-review-fixes-pass-2
kind: review-result
status: active
title: Adversary, story:contracts-review-fixes, pass 2
relations:
- reviews: story:contracts-review-fixes
revision: 1
---
```
unit: story:contracts-review-fixes
verdict: red
cases: executed 444→451, red 2
origin: introduced 1, pre-existing 0, undecided 0
wrote-outside-worktree: <scratch>/adversary-2/ (logs, mutants.sh, replace.py, probe-body.rs; mutant copy and its target/ deleted)
needs-coordinator: finding 1 needs an API decision: a refused acknowledgement should hand the receipt back, or the decision accepts the loss
```

**Verdict: NEEDS-CHANGE.** When `acknowledge_playback` refuses a receipt, the receipt is consumed and lost. A segment whose frames were all played can then never become Played. Everything else in the correction round held.

**Files.** I added two untracked test files and changed nothing else (the diff stat is still 17 files, +1351/−245):
- `crates/inference-tts/tests/adversary2_playback_receipt.rs`
- `crates/inference-testkit/tests/adversary2_sink_without_frames.rs`

**Red cases** (log: `red-tts.log`)

| Case | Line | Result |
|---|---|---|
| `a_receipt_refused_as_wrong_state_still_confirms_the_segment_once_it_is_synthesized` | 94 (assert 122) | `left: Err(PlayedFramesMismatch(PlayedFramesMismatch { segment_id: …, played: 0, emitted: 2 }))  right: Ok(…)` |
| `a_receipt_refused_for_another_segment_still_confirms_its_own` | 133 (assert 161) | `is_ok()` fails: the empty receipt is refused, because A has 2 frames |

**Guard cases** (all green)

| Case | What it checks |
|---|---|
| `adversary2_playback_receipt.rs:170` | An empty receipt is refused for a Queued segment with 0 frames (wrong-state Queued) and for a segment of another synthesis (`SegmentNotHeld`). It is accepted once for a 0-frame Synthesized segment; a second acknowledgement is wrong-state Played. |
| `:222` | A receipt of only the frames streamed while Queued gets `PlayedFramesMismatch{1,2}`. |
| `:246` | A foreign frame in the last position (A, A, B) gets `FrameOfAnotherSegment`. |
| `:266` | A receipt of more frames than emitted gets `PlayedFramesMismatch{3,1}`. |
| `adversary2_sink_without_frames.rs:115` | Through a bare `SynthesisHandle`, `MockSink` acknowledges a 0-frame segment, keeps the frames of another segment, refuses a second acknowledgement, and refuses `NothingGiven` only for a segment that has frames. |

**Gate.** All runs used `CARGO_PROFILE_*_DEBUG=0` and `-p` inference-text, -stt, -tts, -testkit, -llm and stt-only.

| Command | Exit | Result |
|---|---|---|
| `cargo test … --all-features --no-fail-fast` | 101 | 449 passed, 2 failed (the two red cases); `error: 1 target failed` |
| `cargo clippy … --all-targets --all-features -- -D warnings` | 0 | |
| `cargo fmt … --check` | 1, then 0 | Only my two files differed. I ran `rustfmt --edition 2024` on them; the test and clippy runs came before that whitespace-only change. |

The before count of 444 is the after-total minus my 7 cases.

**Mutants.** Each ran on a scratch copy with `cargo test -p inference-tts -p inference-testkit --all-features` (logs `mutant-M*.log`). "Unit" here includes the pass-1 files.

| Mutant | Killed by |
|---|---|
| M1: identity check removed in `acknowledge_playback` | unit `playback_is_acknowledged_only_…` |
| M2: `receipt.frames != emitted` → `<` | **only my `:266`** |
| M3: `!=` → `>` | unit and pass 1 |
| M4: no mixed-segment check in `from_frames` | unit |
| M5: mixed check over the first two frames only | **only my `:246`** |
| M6: MockSink `given == 0 && emitted > 0` → `given == 0` | **only my testkit `:115`** |
| M7: MockSink `NothingGiven` removed | unit `the_sink_refuses_…never_given` |
| M8: empty receipt accepted for any count | unit and pass-1 `only_the_receipt_…` |

**Findings**

1. **A refused acknowledgement destroys the only receipt.** Measured at `crates/inference-tts/src/lib.rs:711`: `receipt` is taken by value, `PlaybackRefused` does not carry it back, and `PlaybackReceipt` is not `Clone`. ESS `AcknowledgePlayback` (`ess/domains/stream.yaml:869`) has a wrong-state outcome that leaves the state unchanged, so the command can run again once the segment is Synthesized. In Rust it cannot. The docs say "nothing changed either way" (`:499`), but the sink's proof of playback is gone.
   - What reaches it: any sink that calls `SynthesisHandle::acknowledge_playback` before the segment is Synthesized (for example while streamed frames arrive ahead of `Completed`), or with the wrong segment id. `MockSink` checks state and counts first (`testkit/src/tts.rs:420-445`), so the mock path never reaches it.
   - At base, acknowledgement went by id and could be retried, so this is introduced.
   - Fix options: return the receipt inside the refusal (`Err((PlaybackRefused, PlaybackReceipt))`), or let `acknowledge_playback` read a `&PlaybackReceipt` (or `&mut`) and spend it only on success, or have the decision accept the loss.
2. **A receipt is tied to a segment id, not to a synthesis.** Note only; I ran it as a probe and deleted it. Synthesis X's two frames confirmed synthesis Y's segment with the same id, which also emitted two, and Y went to Played while its own frames were never played (`probe-cross-synthesis.log`). This only happens with two values of one segment identity, which is the known out-of-scope story:segment-linear-transitions.
3. **`MockSink::acknowledge` says the sink "keeps its frames" on refusal (`testkit/src/tts.rs:413`), but it drains them before calling the handle (`:446-453`).** Note only. It holds for any handle whose `output()` matches what `acknowledge_playback` checks, which includes the mock. I did not run it as a case.

**Pass-1 edits weaken nothing the narrowed decisions still claim.**
- The replaced case (`adversary1_playback_receipt.rs:79`) asserts only `is_err()` for the empty receipt, but the unit test asserts the exact `PlayedFramesMismatch`, and M8 is killed by both.
- The removed half ("the synthesiser cannot confirm its own playback") is now the narrowed decision, and the `compile_fail,E0382` example covers it.
- The two changed expectations in `adversary1_mock_next_step.rs` check the corrected "next step with nothing left to deliver" doc exactly.

**Attacked and could not break**
- An empty receipt for a segment with frames, or for a Queued or foreign segment.
- Segment A's frames passed with B's id when both emitted the same number of frames.
- A partial receipt, including streamed plus completed frames.
- Double acknowledgement.
- The continuing-flow doc (`inference-stt/src/lib.rs:81-87`), which matches `tests/stt.rs:293`.

The lease is released (`session-end` exit 0). No stray processes.

```findings
- file: crates/inference-tts/src/lib.rs
  line: 711
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: acknowledge_playback consumes the PlaybackReceipt on refusal (wrong-state, FrameOfAnotherSegment), so a sink that played every frame can never acknowledge the segment once it is Synthesized; ESS wrong-state changes nothing and the command can be retried. Red at adversary2_playback_receipt.rs:122 and :161
- file: crates/inference-tts/src/lib.rs
  line: 718
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a receipt is tied to a segment id, not to a synthesis; with two values of one identity (out-of-scope story:segment-linear-transitions) synthesis X's frames confirm synthesis Y's segment when the frame counts match (probe-cross-synthesis.log)
- file: crates/inference-testkit/src/tts.rs
  line: 413
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: MockSink docs say the sink keeps its frames on refusal, but it drains them before handle.acknowledge_playback (446-453); holds only for a handle whose output() matches its acknowledge_playback
- file: crates/inference-tts/src/lib.rs
  line: 723
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: mutants != to <, a mixed-segment check over the first two frames only (line 223), and MockSink given==0 alone (testkit tts.rs:436) survive the unit and pass-1 cases; killed only by adversary2 guards :266, :246 and testkit :115
```