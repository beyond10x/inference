---
format: aep.planning-md/3
id: review-result:adversary-llm-contract-pass-1
kind: review-result
status: active
title: Adversary, story:llm-contract, pass 1
relations:
- reviews: story:llm-contract
revision: 1
---
unit: story:llm-contract, uncommitted working tree on base 75df5fb (inference-impl-llm-contract)
verdict: NEEDS-CHANGE
cases: executed 110→115, red 3
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory, <unit scratch>/adversary-1/ (part 6)
needs-coordinator: none

**1. What I touched.** `git --no-pager diff --stat 75df5fb` is unchanged by me: it still shows the implementor's 4 tracked files. My files are untracked, so they show up in `git status --short -uall`:
```
?? crates/inference-llm/tests/adversary1_commit_after_complete.rs
?? crates/inference-testkit/tests/adversary4_llm_admission.rs
?? crates/inference-testkit/tests/adversary4_llm_supersede.rs
```
All three are test files. I touched no non-test path.

**2. Cases added.** I ran each file on its own before running the suite. After those runs I applied `rustfmt` to my own files; that changed layout only.

| file:line | asserts | now |
|---|---|---|
| `crates/inference-llm/tests/adversary1_commit_after_complete.rs:51` | a Tentative segment of a Completed generation commits, because ESS `CommitSegment` has no guard on the generation's state | red |
| `crates/inference-testkit/tests/adversary4_llm_admission.rs:53` | input admitted by a (c) generation is not taken by an (a) handle | red |
| `crates/inference-testkit/tests/adversary4_llm_admission.rs:73` | one admission is reflected once | red |
| `crates/inference-testkit/tests/adversary4_llm_supersede.rs:52` (×2) | supersede or cancel of the mock between a delta and its commit: every segment ends Discarded and nothing more is delivered | green (guard) |

Red output from the runs of these files alone:
```
cargo test -p inference-llm --all-features --test adversary1_commit_after_complete   EXIT=101
  left: Err(GenerationStateConflict { current: Completed })
 right: Ok([SegmentId(617ae8b3-bc2c-4bed-9b38-cc5fe7ac7be8)])
cargo test -p inference-testkit --all-features --test adversary4_llm_admission --test adversary4_llm_supersede   EXIT=101
an adapter claiming output_streaming only took input during output: ["a", "b", "smuggled input"]
assertion `left == right` failed: one admission was accepted 2 times  left: 2 right: 1
cargo test -p inference-testkit --all-features --test adversary4_llm_supersede   EXIT=0 (2 passed)
```
The supersede guard was proved on a mutant copy in scratch. I deleted `self.delivered.clear();` at `crates/inference-testkit/src/llm.rs:184` and `:190`. The unit's `tests/llm.rs` stayed `ok. 3 passed`, and my guard went `FAILED. 0 passed; 2 failed`:
`left: [SegmentCommitted(TextSegmentCommitted { .. text: "a" })] right: []`. The mutant copy and its target/ (594M) are deleted.

**3. Suite run, after my cases existed**
```
cargo test -p inference-llm --all-features --no-fail-fast        EXIT=101  lib 0 | adversary1_commit_after_complete FAILED 0/1 | generation_output ok 5 | doc 0
cargo test -p inference-testkit --all-features --no-fail-fast    EXIT=101  lib 14 | adv1_store 3 | adv1_target 22 | adv2_deferred 19 | adv2_store 2 | adv2_tokens 11 | adv3_sequence 20 | adversary4_llm_admission FAILED 0/2 | adversary4_llm_supersede ok 2 | conform 11 | llm 3 | doc 0
cargo clippy -p inference-llm --all-targets --all-features -- -D warnings       EXIT=0
cargo clippy -p inference-testkit --all-targets --all-features -- -D warnings   EXIT=0
cargo fmt -p inference-llm --check        EXIT=0
cargo fmt -p inference-testkit --check    EXIT=0
```
The "before" count of 110 comes from a second run of the same commands with my 3 files left out: inference-llm `--lib --test generation_output` gave 5; inference-testkit with every other test target listed gave 105. Both exited 0.

**4. Findings** (all against the working tree; all introduced, since neither the crate nor the mock existed at 75df5fb)

- **F1, contract-drift, NEEDS-CHANGE, warning.** `crates/inference-llm/src/lib.rs:222`: `commit_tentative` refuses with `GenerationStateConflict` when the generation is not Active.
  - ESS `inference.stream.CommitSegment` (`ess/domains/stream.yaml:689`) refuses only with `SegmentStateConflict`. The testkit conformance store does the same (`crates/inference-testkit/src/stream.rs:356`).
  - Effect: a segment emitted and not yet committed before `complete()` stays Tentative forever. Commit refuses it, and supersede and cancel refuse a Completed generation, so nothing can move it.
  - The unit's own test pins this drift (`crates/inference-llm/tests/generation_output.rs:136-141`).
  - Measured: red at `adversary1_commit_after_complete.rs:51`.
  - What reaches it: nothing in the tree, because the mock commits before it completes (`src/llm.rs:148-160`). A future adapter that completes before its last commit would hit it.
  - Fix: drop `require_active` from `commit_tentative`. Superseded and cancelled segments are already Discarded, so `commit` refuses them as ESS describes.
- **F2, acceptance, CONFIRMED, warning.** `crates/inference-llm/src/lib.rs:36-37` claims "no implementation accepts input its adapter does not claim". That is false.
  - `AdmittedInput` is not bound to the generation that admitted it, and `GenerationHandle::accept_input` (`:356`) is public, so input admitted by a (c) generation is taken and reflected by an (a) handle.
  - Measured: red at `adversary4_llm_admission.rs:53`.
  - What reaches it: no caller in the tree. Any caller holding two handles can do it, for example one forwarding input from a superseded generation to its successor.
  - Fix: carry the `generation_id` in `AdmittedInput` and have `accept_input` refuse a mismatch, or narrow the doc claim to `offer_input`.
- **F3, boundary, CONFIRMED, note.** `crates/inference-llm/src/lib.rs:156`: `AdmittedInput` derives `Clone`, so one admission can be accepted twice. Measured: red at `adversary4_llm_admission.rs:73`. What reaches it: nothing in the tree. Fix: drop `Clone`.
- **F4, mutant, CONFIRMED, note.** `crates/inference-testkit/src/llm.rs:184` and `:190`: the mock's `supersede` and `cancel` are never called by the unit's suite. The "nothing more is delivered" mutant above survives that suite. My guard now covers it and is green.
- **F5, judgement, CONFIRMED, note.** `crates/inference-testkit/src/lib.rs:35`: the module doc line was edited. The brief's file list does not include this file, and the story's scope says `lib.rs` is not touched. The implementor also left `scratch/testkit-lib-doc.patch` with the same edit. The risk is one more shared line in testkit `lib.rs`, next to the stt and tts lines.

**5. Attacked and not broken**
- `admit_input` checks state before capability, checks the right capability, and refuses at Completed, Cancelled and Superseded. Each mutant would be caught by `generation_output.rs:57-104`; I checked this by reading, not by running the mutants.
- Streaming is observed as a TextDelta while Active for (a) and none for (b). Swapping the mock's `streams` flag either way would be caught by `tests/llm.rs:115-139`; I checked this by reading.
- (c) has no window between the last delta and completion: completion happens only in `next()` with an empty queue.
- Segments are numbered from 1, and supersede or cancel discard both Tentative and Committed segments.
- The "skip a Played segment" branch of `discard_segments` cannot be reached: `GenerationOutput` exposes no queue or play move, so I could not test it.
- `CONTRACT_REVISION` is only read when an adapter is declared; nothing consumes it.

**6. Paths written outside the worktree.** All under `<unit scratch>/adversary-1/`: `red-llm-commit.txt`, `red-testkit-cases.txt`, `run-testkit-supersede.txt`, `mutant-clear.txt`, `gate-test-llm.txt`, `gate-test-testkit.txt`, `before-llm.txt`, `before-testkit.txt`, `gate-clippy-llm.txt`, `gate-clippy-testkit.txt`. I created `mut-clear/` there and deleted it. Lease `f8bfe626-wave5-adv-llm` is released.

**7. Findings block**
```findings
- file: crates/inference-llm/src/lib.rs
  line: 222
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: commit_tentative refuses a Tentative segment of a Completed generation with GenerationStateConflict, where ESS CommitSegment commits it, stranding the segment Tentative with no move left
- file: crates/inference-llm/src/lib.rs
  line: 36
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: AdmittedInput is not bound to its generation, so a handle whose adapter claims only output_streaming takes and reflects input admitted by another generation
- file: crates/inference-llm/src/lib.rs
  line: 156
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: AdmittedInput derives Clone, so one admission is accepted and reflected twice
- file: crates/inference-testkit/src/llm.rs
  line: 184
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the mock's supersede and cancel are never exercised by the unit's suite; removing delivered.clear() survives it, now guarded by adversary4_llm_supersede
- file: crates/inference-testkit/src/lib.rs
  line: 35
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the testkit lib.rs doc line was edited although the brief and the story's scope leave lib.rs untouched
```
