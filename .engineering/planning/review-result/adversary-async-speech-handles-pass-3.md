---
format: aep.planning-md/3
id: review-result:adversary-async-speech-handles-pass-3
kind: review-result
status: active
title: Adversary pass 3 on story:async-speech-handles
relations:
- reviews: story:async-speech-handles
revision: 1
---
unit: story:async-speech-handles pass 3, commit c524f752 (my commit 6fe21e4c on top of it)
verdict: CONFIRMED
cases: executed 87→89, red 1 (normal suites of sherpa-onnx and whisper.cpp: 80→80, because the cases are ignored without a checkpoint; checkpoint-gated run: 7→9)
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 15 new paths in the assigned scratch directory (the pass-2 checkpoint folder is reused); one copied tree there, already deleted
needs-coordinator: whether a sherpa-onnx `next` with nothing offered may wait on the decode gate (finding 1). Strictly it only waits on a dropped recognition's batch, so it may be accepted as designed.

**Verdict:** CONFIRMED, only small findings this pass. The pass-2 whisper.cpp red case had the measure you described; its revision is red on f2ae8019 and green on c524f752. My pass-2 sherpa-onnx cases now pass. The one new red case is small: iterating a fresh sherpa-onnx recognition with nothing offered can block for as long as a dropped recognition's batch decodes.

**1. `git --no-pager diff --stat c524f752 HEAD`**
```
 .../tests/adversary_async_gate_iterate.rs          |  68 +++++++++++++
 .../adversary_async_drop_mid_transcription.rs      | 107 +++++++++++++++------
 2 files changed, 146 insertions(+), 29 deletions(-)
```
Only test files changed.

**2. Cases**

Part 1, `adapters/inference-whispercpp/tests/adversary_async_drop_mid_transcription.rs`, still ignored without the checkpoint:
- A sampler counts threads named `inference-whisp` every millisecond. It first records the largest count during one transcription running alone, as the baseline.
- It then drops a recognition mid-transcription and transcribes the next one to its end.
- It asserts the count never exceeds the baseline plus one; the extra thread is slack for the moment one transcription hands over to the next.
- Run against f2ae8019, in a copy of that commit's files in scratch (no worktree, nothing changed in the tree under attack): red.
  ```
  panicked at adapters/inference-whispercpp/tests/adversary_async_drop_mid_transcription.rs:106:5:
  one transcription alone shows at most 5 threads; after a recognition was dropped mid-transcription the next one admitted ran beside it, 9 threads at once
  test result: FAILED. 0 passed; 1 failed
  ```
- Run on c524f752: `test result: ok. 1 passed`.

Part 2, `adapters/inference-sherpa-onnx/tests/adversary_async_gate_iterate.rs`, ignored without the checkpoint:
- `control_iterating_a_recognition_offered_nothing_returns_at_once`: green.
- `iterating_a_recognition_offered_nothing_does_not_wait_for_another_recognitions_batch`: red now.
  ```
  panicked at adapters/inference-sherpa-onnx/tests/adversary_async_gate_iterate.rs:61:5:
  next on a recognition offered nothing blocked its caller for 11.36516707s, waiting for the dropped recognition's batch to give back its place
  test result: FAILED. 1 passed; 1 failed
  ```
  (Line 63 after formatting.)

**3. Suite run, after the cases existed** (`CARGO_INCREMENTAL=0`, `cargo +1.100.0`)
- `cargo test -p inference-sherpa-onnx --features stt --no-fail-fast`: 39 passed. Exit 0.
- `cargo test -p inference-whispercpp --no-fail-fast`: 41 passed. Exit 0.
- sherpa-onnx with the checkpoint, `--ignored`:
  - `adversary_async_batch_end`: 3 passed. My pass-2 finding is fixed.
  - `adversary_async_gate_iterate`: 1 passed, 1 failed.
  - `stt_async`: 2 passed.
  - Exit 101.
- whisper.cpp with the checkpoint, `--ignored`: `adversary_async_drop_mid_transcription` 1 passed, `stt_async` 1 passed. Exit 0.
- `cargo fmt --all --check`: exit 0. `cargo clippy` on both crates, all targets, `-D warnings`: exit 0.

**4. Findings** (all cover c524f752)

1. **Iterating sherpa-onnx with nothing offered can wait on the decode gate.**
   - Where: `adapters/inference-sherpa-onnx/src/stt.rs:1036`. Only the async form skips a batch of no frames; iterating still takes a place in the decode gate before feeding nothing.
   - Measured: while a dropped recognition's batch holds the only place, `next` on a fresh recognition blocks for 11.4 s. Before c524f752 it returned `None` at once.
   - Who reaches it: a blocking caller that iterates after an earlier recognition was dropped mid-batch. With live recognitions only, a place is always free, so only dropped recognitions' threads can hold every place.
   - Fix (not applied): skip a batch of no frames in the blocking form too.
   - CONFIRMED, introduced.

2. **A panic in a decode thread leaves its async caller waiting forever.**
   - Where: `adapters/inference-sherpa-onnx/src/stt.rs:879`; the same in `adapters/inference-whispercpp/src/stt.rs:689`.
   - The place in the decode gate is given back on unwind, because `_place` is dropped. But the thread wakes the recognition's own waker only on the normal path. A panic disconnects the result channel and nobody wakes the caller, so its `Pending` lasts forever.
   - I found no input that makes `feed` or `transcribe` panic.
   - INFEASIBLE (no reachable panic found), introduced.

3. **whisper-rs 0.16 leaks the abort closure on every async transcription.**
   - Where: `adapters/inference-whispercpp/src/stt.rs:888`, through whisper-rs's `set_abort_callback_safe`.
   - That function `Box::into_raw`s the closure, sets `abort_callback_safe = None`, and `FullParams` has no `Drop`. Each async transcription therefore leaks two boxes and an `Arc<AtomicBool>`.
   - The leak is also why a call after drop is safe: the pointer never dangles.
   - The implementor's boxed-closure workaround matches whisper-rs's cast: the pointer handed over has type `*mut Box<dyn FnMut>`, and the trampoline reads it back as exactly that type.
   - CONFIRMED, introduced; a note.

**5. Attacked and not broken (by reading, except the abort, which is measured)**
- No place in the decode gate leaks on panic: `_place` is dropped on unwind, and on a failed spawn the dropped closure gives it back.
- A waiter is not lost when a place comes back: giving a place back decrements under the lock and wakes every waiter, and `poll_take` checks under the same lock.
- Dropping the recogniser while a decode thread holds a place is safe: the place and the engine each hold their own `Arc`.
- A recognition dropped while `Pending` on the gate leaves only a stale waker, cleared on the next return of a place.
- The sherpa-onnx `advance` loop can neither spin nor starve: after a settle it returns or starts a new batch, and with no frames and input open it returns.
- The whisper.cpp abort is set when the in-flight decode is dropped; the next recognition waits for the gate, as the revised case shows green.
- The testkit's `Failing` recogniser registers its waker before reading the failure flag, and the test checks that it was really woken.

**6. Paths written outside the worktree**
All under `<scratch>/p2/wave3/adversary/`:
- Scripts and messages: `run-cases-p3.sh`, `run-sherpa-p3.sh`, `run-suite-p3.sh`, `commit-msg-p3.txt`
- Logs: `whisper-p3-f2ae8019.log`, `whisper-p3-c524f752.log`, `sherpa-p3.log`, `fmt-p3.log`, `suite-sherpa-p3.log`, `suite-whisper-p3.log`, `ignored-sherpa-p3.log`, `ignored-whisper-p3.log`, `clippy-p3.log`
- Done markers: `cases-p3.done`, `suite-p3.done`
- `tree-f2ae8019/`: the copy of that commit's files with its own build, 409 MB, already deleted.
- `sherpa-checkpoint/` (254 MB, from pass 2): still there.

The worktree's `target/` was 1.5 GB, including the sherpa-onnx native archive; it is deleted. Commit: 6fe21e4c. I released my worktree lease.

**7. Findings block**
```findings
- file: adapters/inference-sherpa-onnx/src/stt.rs
  line: 1036
  category: concurrency
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: iterating takes a decode-gate place for a batch of no frames, so next on a recognition offered nothing blocks its caller while a dropped recognition's batch holds the place, 11.4 s measured (adversary_async_gate_iterate.rs)
- file: adapters/inference-sherpa-onnx/src/stt.rs
  line: 879
  category: concurrency
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a decode thread that panics gives back its gate place but never wakes the recognition's waker, so the async caller stays Pending forever; the same holds at whispercpp stt.rs:689, and no input reaching a panic was found
- file: adapters/inference-whispercpp/src/stt.rs
  line: 888
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: whisper-rs 0.16 set_abort_callback_safe leaks its boxed closure (into_raw, never freed, FullParams has no Drop), so each async transcription leaks two boxes and an Arc<AtomicBool>
```