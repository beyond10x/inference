---
format: aep.planning-md/3
id: review-result:adversary-audio-and-stt-contract-pass-1
kind: review-result
status: active
title: Adversary, story:audio-and-stt-contract, pass 1
relations:
- reviews: story:audio-and-stt-contract
revision: 1
---
unit: story:audio-and-stt-contract, uncommitted working tree on base 9094e40
verdict: INFEASIBLE (1 red case: a state I built myself; no caller in the tree reaches it yet), plus 3 surviving mutants, now caught by my guard cases
cases: executed 170→174, red 1 (inference-testkit, all features; 170 is the 174 my second run executed minus my 4 cases)
origin: introduced 6 / pre-existing 0 / undecided 0
wrote-outside-worktree: 3 paths (part 6)
needs-coordinator: two decisions. Should two recognitions share one stream_id (finding 1)? Is the auto-discovered `stt-only` binary target acceptable (finding 6)?

**1. `git --no-pager diff --stat`**
```
 Cargo.lock | 18 +++ ; Cargo.toml | 2 +- ; crates/inference-testkit/Cargo.toml | 3 +- ;
 crates/inference-testkit/src/lib.rs | 2 +- ; crates/inference-testkit/src/stt.rs | 308 +++
 5 files changed, 330 insertions(+), 3 deletions(-)
```
All of these changes are the implementor's; the stat is the same as before I started. My only changes are two new untracked test files:
- `crates/inference-testkit/tests/adversary7_stt_mock_cues.rs`
- `crates/inference-testkit/tests/adversary7_stt_shared_stream.rs`

I touched no implementation file.

**2. Cases added (each run alone, before the suite)**

| Case | Asserts | Now |
|---|---|---|
| `adversary7_stt_shared_stream.rs::two_recognitions_on_one_stream_obey_the_per_stream_sequence` | Two recognitions on one `stream_id` record cleanly into `EventLog`, and their spans carry distinct sequences | red |
| `adversary7_stt_mock_cues.rs::incremental_runs_each_cue_on_the_frame_that_reaches_its_count` | Incremental mode: frame k delivers exactly cue k, stamped k*160; nothing is left at the end of input | green (guard) |
| `…::a_cue_past_the_offered_frames_runs_at_the_end_of_input` | A cue with `after_frames: 10` and only 2 frames offered still runs at the end of input, in both configurations | green (guard) |
| `…::a_revision_through_the_handle_is_delivered_in_sequence` | `handle.revise` on a Hypothesis delivers a `TranscriptRevision` at sequence 2; the sequences run 1..=7 and `EventLog` accepts them | green (guard) |

Red output, run alone (`cargo test -p inference-testkit --features stt --test adversary7_stt_shared_stream`, EXIT=101):
```
thread 'two_recognitions_on_one_stream_obey_the_per_stream_sequence' panicked at crates/inference-testkit/tests/adversary7_stt_shared_stream.rs:47:43:
turn 1 on the shared stream: sequence 1 on the stream 323a77fc-… is not greater than its last, 6
test result: FAILED. 0 passed; 1 failed
```
The guard file run alone: `test result: ok. 3 passed`, EXIT=0.

Mutants, run in a scratch copy of the tree (copy deleted). Each one passes the unit's own `tests/stt.rs` (4 passed, EXIT=0) and fails a guard (EXIT=101):

| Mutant in `crates/inference-testkit/src/stt.rs` | Caught by |
|---|---|
| M1 `:255` `>` → `>=` (each cue runs one frame late) | `incremental_runs_each_cue…`, `a_revision_through_the_handle…` |
| M2 `:255` drop `!ended &&` (at end of input, cues past the frame count never run) | `a_cue_past_the_offered_frames…` |
| M3 `:306` drop `self.delivered.push_back(envelope)` (a handle revision is lost, but its sequence is still used) | `a_revision_through_the_handle…` |

**3. Suite (run after the cases existed)**
- `cargo test -p inference-testkit --all-features --no-fail-fast`: EXIT=101. Every binary passed except `adversary7_stt_shared_stream` (`FAILED. 0 passed; 1 failed`). `adversary7_stt_mock_cues` ok, 3 passed; `stt` ok, 4 passed.
- `cargo clippy -p inference-testkit --all-targets --all-features -- -D warnings`: EXIT=0.
- `cargo fmt -p inference-testkit --check`: EXIT=0.
- I did not re-run the suites of `inference-stt` or `stt-only`, because I added no files there.

**4. Findings (all cover the working tree on base 9094e40)**

1. **Sequence restarts at 1 for each recognition** (`crates/inference-stt/src/lib.rs:267`, `:433-436`). INFEASIBLE, introduced.
   - **What was measured:** two recognitions on one caller-supplied `stream_id` both write sequence 1. `EventLog` refuses the second with SequenceNotIncreasing, and two spans on that stream share a sequence, which `ess/domains/stream.yaml` (EmitHypothesis, UNMAPPED note) says is to be refused.
   - **What reaches it:** nothing found. The example and the tests generate a fresh StreamId for every recognition, and no graph or runtime exists yet. A node recognising several turns on one output port would reach it.
   - **Fix options:** document "one stream per recognition" on `RecognitionRequest`, or carry the stream's sequence across recognitions.
2. **Mock: M1 survived the unit's suite** (`crates/inference-testkit/src/stt.rs:255`). CONFIRMED, introduced. The module doc's "runs every cue whose frame count is reached" was untested. Guarded now.
3. **Mock: M2 survived** (`stt.rs:255`). CONFIRMED, introduced. "At the end of input every cue left runs" was untested, because `script()` fits its six frames exactly. Guarded now.
4. **Mock: M3 survived** (`stt.rs:304-307`). CONFIRMED, introduced. A successful revision through the handle was never asserted; only the refused path was. Guarded now.
5. **Doc overclaims line 362** (`crates/inference-stt/src/lib.rs:22-23`). NEEDS-CHANGE, note, introduced. The doc says a chunked offline recogniser "cannot claim incremental recognition and take input early anyway". The contract only stops an adapter that does not claim the capability from taking input early. It cannot see an adapter that claims IncrementalInput and processes chunks in independent calls, which is the case design line 362 forbids. The wording needs correcting.
6. **The example is a binary target** (`examples/stt-only/src/main.rs`). INFEASIBLE, note, introduced. Cargo turns `src/main.rs` into a bin target named `stt-only`; `tests/output.rs` relies on `CARGO_BIN_EXE_stt-only`. This keeps the letter of the brief (no `[[bin]]`, publish = false), but AGENTS.md:55 says "no binary". Coordinator's call.
7. **Out-of-scope edit** (`crates/inference-testkit/src/lib.rs:31`). CONFIRMED, note, introduced. A one-line doc-comment change. The story's Scope says "Not touched: crates/inference-testkit/src/lib.rs", and the brief lists other testkit files as not the implementor's.
8. **Tree test is mostly vacuous** (`examples/stt-only/tests/tree.rs:34-38`). CONFIRMED, note, introduced. `inference-tts` and `inference-runtime` are not workspace members, and `-e normal` already drops the testkit's dev-only `ess-conformance`. So only the `inference-llm` assertion can currently fail. It matches story check 3 as written.

**5. Attacked, could not break**
- The capability gates are crossed correctly in the unit's tests: swapping either capability, or dropping `input_ended ||`, is caught.
- The timestamp is `position + sample_count`, and a span's sequence equals its hypothesis envelope's.
- Kind names match `stream.yaml:935-958`, and the envelopes pass every `EventLog` rule within one recognition, including a revision through the handle.
- §23.1 test 1: the refusal and the unchanged text hold in both configurations.
- The mock never reads its own claims (`stt.rs:251-281`).
- The `Cargo.lock` delta adds only inference-stt, stt-only and the testkit edge.

**6. Paths written outside the worktree** (the mutant copy `…/adversary-1/mutant/`, with its target/, was deleted)
- <unit scratch>/adversary-1/mutants.sh
- <unit scratch>/adversary-1/testkit.log
- <unit scratch>/adversary-1/clippy.log

**7. Findings block**
```findings
- file: crates/inference-stt/src/lib.rs
  line: 267
  category: contract-drift
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: every recognition numbers envelopes and spans from 1 on a caller-supplied stream_id, so two recognitions on one stream violate the per-stream sequence rule the EventLog enforces; no caller reaches it yet
- file: crates/inference-testkit/src/stt.rs
  line: 255
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: flipping > to >= makes every cue run one frame late and the unit's suite stays green; adversary7_stt_mock_cues now catches it
- file: crates/inference-testkit/src/stt.rs
  line: 255
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: dropping the end-of-input override leaves cues past the offered frame count unrun and the unit's suite stays green; adversary7_stt_mock_cues now catches it
- file: crates/inference-testkit/src/stt.rs
  line: 306
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: dropping delivery of a handle revision loses the event while its sequence is used and the unit's suite stays green; adversary7_stt_mock_cues now catches it
- file: crates/inference-stt/src/lib.rs
  line: 22
  category: judgement
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the doc says the contract stops a chunked offline recogniser claiming incremental recognition, but it only stops adapters that do not claim the capability, which is not the design line 362 case
- file: examples/stt-only/src/main.rs
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: src/main.rs is auto-discovered as a bin target named stt-only, which AGENTS.md's no-binary rule may not intend for an example
- file: crates/inference-testkit/src/lib.rs
  line: 31
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the unit edited a file its story's Scope lists as not touched and its brief does not assign
- file: examples/stt-only/tests/tree.rs
  line: 34
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: three of the four absence assertions name crates that cannot appear in the normal-edge tree today, so only the inference-llm assertion can fail
```
