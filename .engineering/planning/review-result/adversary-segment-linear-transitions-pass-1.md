---
format: aep.planning-md/3
id: review-result:adversary-segment-linear-transitions-pass-1
kind: review-result
status: active
title: Adversary, story:segment-linear-transitions, pass 1
relations:
- reviews: story:segment-linear-transitions
revision: 1
---
unit: story:segment-linear-transitions, working tree on base 010843f with the unit's uncommitted change and my 4 new test files
verdict: CONFIRMED (no blocker)
cases: executed 581→592, red 0
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory, home/timo/.cache/inference-wave-12/segment-linear-transitions/scratch/adversary-1/ (mutant copy already deleted)
needs-coordinator: none

Coordinator: I had not stopped. The tts case file was written and run, and the whole pass finished. The lease was alive at the re-check heartbeat and is now released with `session-end`.

**1. `git --no-pager diff --stat`**: the same 13 files as the unit's change; I edited none of them. Untracked, mine, all test files:
- `crates/inference-text/tests/adversary1_linear_transitions.rs`
- `crates/inference-tts/tests/adversary1_linear_transitions.rs`
- `crates/inference-testkit/tests/adversary1_segment_store.rs`
- `crates/inference-testkit/tests/adversary1_llm_concurrency.rs`

No file outside the tests was touched.

**2. Cases added (11, all green now, each run alone first and green there)**

| File | Cases | What they assert | Mutants that make them red |
|---|---|---|---|
| text adversary1 | 2 | The commit-twice doctest compiles once its second commit is removed. On stable, rustdoc does not check `E0382`, so this guard proves the doctest fails for the right reason. A refusal displays as its conflict, gives it as `source()`, and hands the segment back unchanged. | n/a |
| tts adversary1 | 4 | The second crate doctest compiles once its second value is removed. `complete` reports wrong-state before checking frames. `acknowledge_playback` reports wrong-state before checking the receipt. Every move and refusal keeps segment order and per-segment frame counts. | T1, T2, T3 |
| testkit segment_store | 2 | A refused move keeps the slot and records no event. `cancel_run` and supersede keep Played and Discarded segments in place and in order. | S1, S2, S3 |
| testkit llm_concurrency | 3 | A Completed, Cancelled or Superseded generation holds its slot until its handle is dropped. The slot outlives the generator it came from. 8 clones racing on threads, 50 rounds: exactly 1 is admitted. | n/a |

Red output from the mutant copy (verbatim, copy since deleted):
```
T1 tts:127  left: Err(FrameLengthMismatch(FrameLengthMismatch { expected: 4, actual: 3 }))  right: Err(SegmentStateConflict(SegmentStateConflict { current: Synthesized }))
T2 tts:156  left: Err(FrameOfAnotherSegment(..))  right: Err(SegmentStateConflict(SegmentStateConflict { current: Queued }))
S1 store:68 assertion failed: a refused move changes no slot (store order 5,3,4,2 vs 2,5,3,4)
```

| Mutant | Change | Unit's suite | Mine |
|---|---|---|---|
| T1 | remove `require` at `tts/src/lib.rs:693` | **survives** | red |
| T2 | remove `require` at `tts/src/lib.rs:755` | **survives** | red |
| T3 | tts `move_segment` uses push instead of insert | red (2 cases) | red |
| S1 | `stream.rs:1086` uses push instead of insert | **survives, conform included** | red |
| S2 | `discard_where` drops Played segments | red (11 targets) | red |
| S3 | a refused move loses its segment | red (6 targets) | red |

**3. Gate after my cases** (each command run separately, exit status per command)

| Crate | test | clippy | fmt | passed |
|---|---|---|---|---|
| inference-text | 0 | 0 | 0 | 21 |
| inference-tts | 0 | 0 | 0 | 29 |
| inference-llm | 0 | 0 | 0 | 20 |
| inference-testkit | 0 | 0 | 0 | 522 |

- `cargo check -p inference-stt -p stt-only`: exit 0.
- Conform: `counts: {"error":0,"failed":0,"passed":197,"skipped":0,"total":231,"unsupported":34}`, unchanged.
- The first clippy and fmt run failed on my own files only: `type_complexity`, plus formatting. I fixed them with a type alias and `rustfmt` on my 4 files.
- The 581 "before" figure comes from the same run: 592 minus my 11 cases.

**4. Findings** (all on the working tree above)

1. `crates/inference-text/src/lib.rs:16` and `:311-312`: the docs say a segment's "identity yields at most one Committed value", with no exception.
   - **Measured:** `OutputSegment::emit_as` (`:227-235`) takes any `SegmentId`. The unit's own `synthesis_output.rs` uses it to build two Committed values of one identity.
   - **Reaches it:** any caller of `emit_as`. The inference-tts crate doc names this route, as you asked; inference-text and the `emit_as` doc do not.
   - **Fix:** qualify the claim to "through `emit`" and name the `emit_as` route in the `emit_as` doc.
2. Mutants T1 and T2 at `tts/src/lib.rs:693` and `:755`: nothing in the unit's suite pins the documented refusal order. My two tts order cases do.
   - **Reaches it:** a caller completing an already Synthesized or Discarded segment with a bad frame, or acknowledging a Queued segment with a wrong receipt.
3. Mutant S1 at `testkit/src/stream.rs:1086`: the store and the `Segments` view promise emission order ("in the order the segments were emitted", `:178`, `:1180`), and nothing pinned it, conform included. `adversary1_segment_store` pins it now.
4. `crates/inference-llm/src/lib.rs:236`: a 170-character doc line from the unit's edit. rustfmt does not wrap comments, so fmt passes.

**5. Attacked and could not break**
- Every transition takes the segment by value. The store and tts `move_segment` signatures, `fn(OutputSegment) -> …`, make a revert to `&self` fail to compile.
- A refusal hands the segment back unchanged, in the stream store, in the tts store and in `discard_segments` (mutants S2 and S3 are killed).
- `commit_tentative` stopping part-way can't happen: only Tentative segments are selected and commit refuses only non-Tentative ones. If it ever did, the earlier commits would stay and their events would be dropped; that path is not reached.
- `Generator::start` returns `Result`; dependents compile.
- The LLM mock slot is released only when the handle is dropped. Finish, cancel and error keep it while the handle is held, matching the STT and TTS mocks and the docs. Clones share it, a refused start leaves no count behind, and the thread race admits exactly 1.
- `complete` and `acknowledge_playback` refuse in the same order as at base.

**6. Paths written outside the worktree**: `<unit scratch>/adversary-1/`, holding the gate-*.log, mutant-{T1,T2,T3,S1,S2,S3}.log and red-{T1,T2,S1}.txt files plus `mutate.py` and `run-mutant.sh`. I used one mutant copy, `mutant/`, for all six mutants, rebuilding into the same `target/` to save disk; it was deleted after the last run. Free disk on `/` is 12G, down from 19G at start, while the other two units build.

```findings
- file: crates/inference-text/src/lib.rs
  line: 16
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: crate and commit docs (also :311-312) claim an identity yields at most one Committed value without naming the emit_as route that yields two, which the inference-tts doc does name
- file: crates/inference-tts/src/lib.rs
  line: 693
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: deleting the Queued require in complete survives the unit's suite (FrameLengthMismatch reported before wrong-state); guarded by adversary1_linear_transitions
- file: crates/inference-tts/src/lib.rs
  line: 755
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: deleting the Synthesized require in acknowledge_playback survives the unit's suite (FrameOfAnotherSegment reported before wrong-state); guarded by adversary1_linear_transitions
- file: crates/inference-testkit/src/stream.rs
  line: 1086
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: replacing insert(index) with push reorders the store and the Segments view against their documented emission order and survives the whole testkit suite including conform; guarded by adversary1_segment_store
- file: crates/inference-llm/src/lib.rs
  line: 236
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the edited GenerationOutput doc line runs to 170 characters, unwrapped
```