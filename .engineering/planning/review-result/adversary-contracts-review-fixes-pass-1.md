---
format: aep.planning-md/3
id: review-result:adversary-contracts-review-fixes-pass-1
kind: review-result
status: active
title: Adversary, story:contracts-review-fixes, pass 1
relations:
- reviews: story:contracts-review-fixes
revision: 1
---
unit: story:contracts-review-fixes, uncommitted working tree on base 5f40904 plus 2 new adversary test files
verdict: NEEDS-CHANGE
cases: executed 436→441, red 4
origin: introduced 3 / pre-existing 1 / undecided 0
wrote-outside-worktree: 1 path (scratch/adversary-1/, 6 logs)
needs-coordinator: finding 1 is pre-existing, but it is this story's decision 4 left unfinished, so it should stay with this unit and not be routed out

**1. Diff stat.** `git --no-pager diff --stat` is unchanged from the start: the same 17 files and 1238+/242- that the unit and your callers patch made. I touched none of them. My only changes are two untracked test files:
- `crates/inference-tts/tests/adversary1_playback_receipt.rs`
- `crates/inference-testkit/tests/adversary1_mock_next_step.rs`

No file outside tests changed.

**2. Cases added.** Each file was run alone before the suite (logs `red-tts.log`, `red-testkit.log`).

| Case | Asserts | Now |
|---|---|---|
| `adversary1_playback_receipt.rs:73` `the_synthesiser_that_completes_a_segment_cannot_confirm_its_own_playback` | the owner of `SynthesisOutput` calls `complete`, keeps the frames, makes a receipt from them; acknowledging must be refused | red |
| `adversary1_playback_receipt.rs:106` `a_segment_synthesized_without_frames_can_still_be_acknowledged` | `complete(id, vec![])` leaves the segment Synthesized with 0 frames; a sink must still be able to make a receipt | red |
| `adversary1_mock_next_step.rs:50` `the_mock_recogniser_takes_the_offered_frames_at_its_next_step` | the corrected STT mock doc ("takes the frames offered so far at its next step, when the handle is next iterated") | red |
| `adversary1_mock_next_step.rs:83` `the_mock_synthesiser_takes_the_offered_segments_at_its_next_step` | the same claim in the TTS mock doc, for segments | red |
| `adversary1_mock_next_step.rs:116` `the_mock_recogniser_stops_at_the_last_sequence_of_its_stream_without_a_panic` | guard: a mock continuing at `i64::MAX - 1` delivers exactly MAX-1 and MAX, then nothing; a handle `revise` returns `SequenceExhausted`; no panic | green |

Red output, verbatim:
```
panicked at crates/inference-tts/tests/adversary1_playback_receipt.rs:89:5:
the synthesiser confirmed playback of frames no sink was given: Ok(AudioPlayed { segment_id: SegmentId(bb84e5cb-...) })
panicked at crates/inference-tts/tests/adversary1_playback_receipt.rs:123:9:
[]: the sink played all of the segment's audio, none, and cannot make a receipt of it: Err(NoFramesPlayed(NoFramesPlayed)); the Synthesized segment can never be Played
panicked at crates/inference-testkit/tests/adversary1_mock_next_step.rs:66:5:
assertion `left == right` failed: the handle was next iterated, and the frame offered before it was not taken
  left: 2
 right: 3
panicked at crates/inference-testkit/tests/adversary1_mock_next_step.rs:96:5:
assertion `left == right` failed: the handle was next iterated, and the segment offered before it was not taken
  left: 1
 right: 2
```
The line numbers above are from before `rustfmt`. The assertions are now at `:90`, `:128`, `:73` and `:106`.

**Mutant for the guard case.** In a scratch copy, `src/stt.rs:365` `Err(CommitRefused::SequenceExhausted(_)) => return` was changed to `panic!`, then `cargo test -p inference-testkit --all-features --no-fail-fast` ran on that copy: 373 passed, 3 failed, ending `error: 1 target failed`. All 3 failures are in my file. The unit's own cases (`tests/stt.rs` and the adversary7 files) stayed green, so they do not cover the mock reaching the end of its sequence. The copy and its `target/` are deleted.

**3. Gate**, run after the cases existed (`CARGO_PROFILE_*_DEBUG=0`, packages inference-text, -stt, -tts, -testkit, -llm and stt-only):

| Command | Exit | Result |
|---|---|---|
| `cargo test … --all-features --no-fail-fast` | 101 | 437 passed, 4 failed (the 4 red cases above); `error: 2 targets failed` |
| `cargo clippy … --all-targets --all-features -- -D warnings` | 0 | |
| `cargo fmt … --check` | 1, then 0 | the first failure was only my two files; I ran `rustfmt` on those two and re-ran |

The before count of 436 is the after-run total minus my 5 cases. I did not run the suite before writing the cases.

**4. Findings** (they cover the working tree on 5f40904)

1. **Decision 4 is not met: the synthesiser can still confirm its own playback.**
   - Measured: `crates/inference-tts/src/lib.rs:707` (`acknowledge_playback`) and `:204` (`PlaybackReceipt::from_frames`). Red at `adversary1_playback_receipt.rs:90`.
   - What reaches it: every `Synthesiser` implementation. It is the only caller of `complete` and `audio`, so it holds every `SegmentAudio` before any sink does.
   - The crate docs are honest about this: they say a synthesiser "that delivered its frames" cannot. The story's decision ("a value only a sink can make") claims more.
   - At base the same self-confirmation worked with just a segment id.
   - Fix options: narrow the decision to what the docs say, or bind the receipt to something the sink holds and the synthesiser never sees.
   - CONFIRMED, pre-existing.
2. **A segment completed with zero frames can never be Played.**
   - Measured: `crates/inference-tts/src/lib.rs:665` (`complete` accepts an empty `Vec`; its doc says a streaming synthesiser "may pass none", and nothing refuses it otherwise) and `:204` (an empty receipt is refused with `NoFramesPlayed`). Red at `adversary1_playback_receipt.rs:128`.
   - ESS `AcknowledgePlayback` (`ess/domains/stream.yaml:869`) gives every Synthesized segment a `played` outcome.
   - What reaches it: any adapter that completes with no frames, for example for empty text. The mock never does, because it always emits at least one frame.
   - At base the segment could be acknowledged by id, so this is new.
   - Fix options: refuse `complete` when it would leave a segment with 0 frames emitted, or accept a zero-frame acknowledgement for a segment that emitted 0.
   - NEEDS-CHANGE, introduced.
3. **The corrected mock docs still do not describe what the code does.**
   - Measured: `crates/inference-testkit/src/stt.rs:11` against `:324` (input is taken only when `delivered` is empty), and `src/tts.rs:12` against `:237`. Red at `adversary1_mock_next_step.rs:73` and `:106`.
   - What reaches it: any caller that offers input before draining the handle. The unit's own `recognise` and `synthesise` helpers drain after every offer, so they never see it.
   - Fix: change the doc to "at the next step that has nothing left to deliver".
   - NEEDS-CHANGE, introduced.
4. **AcknowledgePlayback has a refusal ESS does not declare.**
   - `inference-tts/src/lib.rs:510` adds `PlayedFramesMismatch`; ESS declares only `played` and `wrong-state`.
   - What reaches it: the conformance target still acknowledges by segment id (`tests/target/stream.rs:174`), so no conformance id sees it.
   - Not run as a case. Note only.
   - CONFIRMED, introduced.
5. **Note, not run as a case.** The documented continuing-stream flow ("start the next recognition at `next_sequence` of the previous one", `inference-stt/src/lib.rs:83`) is refused with `ConcurrencyLimitReached` when the caller still holds the previous handle. The slot is released only on drop (`stt.rs:228` and `Slot::drop`). The docs say so, so this is judgement, not a red case.

**5. Attacked and could not break**
- `continuing` refuses 0, -1 and `i64::MIN` and accepts 1 and `i64::MAX`.
- The checked advance: a `saturating_add` mutant is caught by the unit's own test.
- No panic at `i64::MAX`, in the contract and in the mock (guard case).
- Nothing moves on any refusal: hypothesis, revise and commit all check the sequence before moving a span.
- Concurrency slot:
  - a slot leaked on a refused start (the `forget` mutant) is caught by the unit's test;
  - an off-by-one limit (`<=`) is caught by `observed_concurrency`;
  - clones share the limit;
  - drop and unwind release the slot.
- The receipt refuses frames of another segment, an empty set and a partial set.
- `SegmentAudio` fields are private, and no `Clone`, `Default` or `Deserialize` path builds one or `PlaybackReceipt`. `SynthesisEvent` has no `Clone`.
- The doc correction for `complete` matches the code: `audio` and `complete` both refuse once the segment is Synthesized.
- Every caller of `start` handles the `Result`: the example, the llm tests and the testkit tests.

**6. Written outside the worktree.** In `<unit scratch>/adversary-1/`: `red-tts.log`, `red-testkit.log`, `suite.log`, `clippy.log`, `fmt.log`, `mutant-mock-commit.log`. The mutant copy and its `target/` are deleted. The lease is released (`session-end` exit 0).

```findings
- file: crates/inference-tts/src/lib.rs
  line: 707
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: pre-existing
  message: the code that calls complete holds the frames and can make a PlaybackReceipt and confirm its own playback, so decision 4 ("a value only a sink can make") holds only for a synthesiser that delivers its frames
- file: crates/inference-tts/src/lib.rs
  line: 665
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: complete accepts zero frames, but a receipt needs at least one, so such a Synthesized segment can never take the played outcome ESS gives it
- file: crates/inference-testkit/src/stt.rs
  line: 11
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the corrected STT and TTS mock docs say input is taken when the handle is next iterated, but it is taken only when no event is waiting (stt.rs:324, tts.rs:237)
- file: crates/inference-tts/src/lib.rs
  line: 510
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: AcknowledgePlayback gains a PlayedFramesMismatch refusal that ess/domains/stream.yaml does not declare (only played and wrong-state)
- file: crates/inference-stt/src/lib.rs
  line: 83
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the documented continuation flow reads next_sequence from the previous handle, and start refuses while that handle is still held, because a slot is released only on drop
```