---
format: aep.planning-md/3
id: review-result:adversary-tts-contract-pass-1
kind: review-result
status: active
title: Adversary, story:tts-contract, pass 1
relations:
- reviews: story:tts-contract
revision: 1
---
```
unit: story:tts-contract, uncommitted working tree on base 4a96264 (impl/tts-contract)
verdict: NEEDS-CHANGE
cases: executed 269→275, red 3
origin: introduced 7 / pre-existing 0 / undecided 0
wrote-outside-worktree: 18 files in <unit scratch>/adversary-1/ (listed in part 6); the mutant copy there is deleted
needs-coordinator: F1 needs a design decision: either the synthesis record learns about playback, or the acceptance's "then refuses discard" is narrowed to the sink's copy
```

**1. Diff stat** (`git --no-pager diff --stat 4a96264`): the same 5 tracked files the implementor changed (330+, 2−). I changed no tracked file. My 4 files are untracked and all are test files:
`crates/inference-tts/tests/adversary1_segment_identity.rs`, `crates/inference-testkit/tests/adversary9_tts_played_record.rs`, `crates/inference-testkit/tests/adversary9_tts_empty_text.rs`, `crates/inference-audio/tests/adversary3_frame_width.rs`. The modified `crates/inference-testkit/src/lib.rs` is the implementor's edit (see F4), not mine.

**2. Cases** (each run alone before the suite; logs `case-*.log`)

| Case | Asserts | Now |
|---|---|---|
| adversary9_tts_played_record.rs:61 | after `MockSink::acknowledge`, the synthesis's record of the segment refuses discard | RED |
| adversary1_segment_identity.rs:51 | a second synthesis refuses a segment the first one already queued (`current: Queued`) | RED |
| adversary1_segment_identity.rs:74 | a segment the caller discarded after offering it does not complete synthesis | RED |
| adversary1_segment_identity.rs:92 | `complete` on a segment offered but not yet taken returns `SegmentNotTaken` | green guard, kills mutant M2 |
| adversary9_tts_empty_text.rs:50 | a segment with no words (`""`, `"   "`) gets 1 frame and can be played | green guard, kills M3 |
| adversary3_frame_width.rs:35 | `data_len` matches literal values for both encodings, up to `u16::MAX × u32::MAX` | green guard, kills M1 |

Red output as first captured:
```
played_record.rs:61: IncrementalStreaming: after AcknowledgePlayback the synthesis still holds the segment as Synthesized ...
  left: Ok(Discarded)   right: Err(SegmentStateConflict { current: Played })
segment_identity.rs:51: ... the caller's copy is still Committed
  left: Ok(SegmentQueued { segment_id: SegmentId(95c14fb8-…) })   right: Err(SegmentStateConflict(SegmentStateConflict { current: Queued }))
segment_identity.rs:74: a Discarded segment is terminal and CompleteSynthesis moves only a Queued one
  left: Ok(())   right: Err(SegmentStateConflict(SegmentStateConflict { current: Discarded }))
```

Mutants, each run in a scratch copy of the workspace; that copy (534M with its target) is deleted:
- **M1** (`PcmF32le` width 4→2): the inference-audio and inference-tts suites stay green (13+4 pass). Only the testkit's `tts.rs` and my case fail.
- **M2** (`complete` searches all segments, not only taken ones): the unit's suites stay green (tts 4/4, testkit `tts` 7/7). Only my case fails.
- **M3** (drop `.max(1)` in the mock): testkit `tts` stays green 7/7. Only my case fails.

**3. Suite** (after the cases existed; `gate.sh`, `CARGO_PROFILE_*_DEBUG=0`; my clippy/fmt slips in my own files fixed and re-run)
```
test inference-tts EXIT=101      passed=6 failed=2   clippy EXIT=0  fmt EXIT=0
test inference-audio EXIT=0      passed=22 failed=0  clippy EXIT=0  fmt EXIT=0
test inference-testkit EXIT=101  passed=244 failed=1 clippy EXIT=0  fmt EXIT=0
```
- `executed` before: 269 (243 + 5 + 21), from the implementor's gate logs.
- After: 275 (245 + 8 + 22).

**4. Findings**

- **F1** (acceptance, blocker, NEEDS-CHANGE, introduced)
  - Where: `crates/inference-tts/src/lib.rs:295`. `SynthesisOutput` keeps its own copy of each segment, and no path moves that copy to Played.
  - Measured: once the sink has acknowledged, `handle.output().segment(id)` still reports Synthesized and accepts discard, in all 4 configurations (played_record.rs:61).
  - The unit's own test asserts this at `crates/inference-testkit/tests/tts.rs:380-383` with the message "a Synthesized segment not yet played is still discarded", though it has just been played.
  - Spec: ESS makes Played terminal; DiscardSegment "cannot unsay what the user heard". The acceptance says the segment "then refuses discard". That holds only for the separate copy the sink returns.
  - What reaches it: the unit's acceptance test builds exactly this sequence. No runtime consumer of inference-tts exists in the tree.
  - Fix options: let the playback acknowledgement update the synthesis record (with the sink as the actor), or keep one shared record of each segment.
- **F2** (contract-drift, warning, INFEASIBLE, introduced)
  - Where: `lib.rs:332-345`. `offer(&segment)` queues a copy and leaves the caller's segment Committed.
  - Measured: a second synthesis queues the same segment again (:51). A segment the caller discarded is still synthesised to completion (:74).
  - The claim "a segment offered twice is refused (current Queued)" therefore holds only within one synthesis.
  - The contract also has no way to discard a segment the synthesis holds.
  - What reaches it: nothing in the tree; I built both cases. Same root cause as F1.
- **F3** (mutant, note, CONFIRMED, introduced): `lib.rs:420` (`complete` only works on taken segments), `inference-audio/src/lib.rs:31-35,55-57` (width and `data_len`) and the mock's `.max(1)` at `testkit/src/tts.rs:141` are not pinned by any case of the unit. M1 is caught only at workspace level, by the testkit. My 3 guard cases now pin all three.
- **F4** (judgement, warning, CONFIRMED, introduced)
  - Where: `crates/inference-testkit/src/lib.rs:40-41`, which the brief lists as "not yours".
  - The implementor wrote the doc-line patch to scratch (`testkit-lib-doc.patch`, 09:36:02) and then also applied it to the tree (file mtime 09:36:48).
- **F5** (judgement, note, CONFIRMED, introduced): `MockSink::acknowledge` (`testkit/src/tts.rs:272-292`) accepts playback after 1 of N frames. In the configurations without `output_streaming`, the synthesis then keeps emitting frames of a segment that is already Played.

**5. Attacked, not broken**
- Tentative refusal in all 4 configurations.
- Duplicate offer within one synthesis, including when the held copy is Synthesized.
- Offer and end after the utterance ended.
- `take_input` with and without `incremental_input`.
- The `output_streaming` gate on Queued segments.
- Frame length at 0 bytes and off by one.
- Overflow of `data_len`: the maximum is about 1.126e15, below 2^50.
- The mock's sample clock and job bookkeeping across interleaved offers.
- Serialized names against audio.yaml (unchanged).

**6. Paths written outside the worktree**, all under `<unit scratch>/adversary-1/`:
- `gate.sh`
- `case-audio.log`, `case-testkit.log`, `case-tts.log`
- `gate-test-inference-{tts,audio,testkit}.log`
- `gate-clippy-inference-{tts,audio,testkit}.log`
- `gate-fmt-inference-{tts,audio,testkit}.log`
- `m1-own.log`, `m1-testkit.log`, `m2-tts.log`, `m2-testkit.log`, `m3-testkit.log`
- `mutant/`: deleted.

The lease `f8bfe626-wave8-adv-tts` is released.

```findings
- file: crates/inference-tts/src/lib.rs
  line: 295
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: after the sink acknowledges playback the synthesis still holds the segment as Synthesized and accepts discard (adversary9_tts_played_record.rs:61 red), and the unit's tts.rs:380-383 asserts that as correct
- file: crates/inference-tts/src/lib.rs
  line: 332
  category: contract-drift
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: offer queues a copy and leaves the caller's segment Committed, so a second synthesis queues it again and a discarded segment still completes synthesis (adversary1_segment_identity.rs:51 and :74 red); no caller in the tree reaches this
- file: crates/inference-tts/src/lib.rs
  line: 420
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the taken-only rule of complete, the encoding width and data_len, and the mock's one-frame minimum survive mutants M2, M1 and M3 under the unit's package suites; guard cases added
- file: crates/inference-testkit/src/lib.rs
  line: 40
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the implementor edited a file the brief marks not-theirs, after also writing the same change to the scratch patch
- file: crates/inference-testkit/src/tts.rs
  line: 272
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: MockSink acknowledges playback after the first of several frames, so without output_streaming the frames of an already Played segment are still emitted afterwards
```
