---
format: aep.planning-md/3
id: review-result:adversary-async-speech-handles-pass-2
kind: review-result
status: active
title: Adversary pass 2 on story:async-speech-handles
relations:
- reviews: story:async-speech-handles
revision: 1
---
unit: story:async-speech-handles pass 2, commit f2ae8019 (the new tests are commit 55e56766 on top of it)
verdict: NEEDS-CHANGE
cases: executed 82→86, red 3 (normal suites of sherpa-onnx and whisper.cpp: 80→80, because the new cases are ignored without a checkpoint; checkpoint-gated run: 2→6)
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 21 paths, all in the assigned scratch directory (one of them is the 254 MB sherpa-onnx checkpoint folder)
needs-coordinator: the new cases are `#[ignore]` without a checkpoint, like the adapters' own conformance tests, so CI never runs them. Whoever re-runs them needs the checkpoint paths in part 6.

**Verdict:** NEEDS-CHANGE. With sherpa-onnx, if input ends while a batch is still decoding, the usage record never arrives. With whisper.cpp, dropping a recognition mid-transcription leaves its thread running past the one-at-a-time limit. The pass-1 OpenAI cases now pass, 4 of 4.

**1. `git --no-pager diff --stat f2ae8019 HEAD`**
```
 .../tests/adversary_async_batch_end.rs             | 128 +++++++++++++++++++++
 .../adversary_async_drop_mid_transcription.rs      |  62 ++++++++++
 2 files changed, 190 insertions(+)
```
Only test files changed.

**2. Cases added.** Each was run alone with its checkpoint (pinned sherpa-onnx checkpoint fetched and hash-checked; the `ggml-base.en.bin` already on this machine matches the pin).

`adapters/inference-sherpa-onnx/tests/adversary_async_batch_end.rs`:
- `control_the_async_form_delivers_the_usage_when_input_ends_before_the_first_poll`: green. The same two seconds of silence, with input ended before the first poll, delivers the usage record.
- `the_async_form_delivers_the_usage_when_input_ends_while_a_batch_decodes`: red now.
- `iterating_delivers_the_usage_when_input_ended_while_an_async_batch_decoded`: red now.

Red output from the first run, which held only the two red cases:
```
panicked at adapters/inference-sherpa-onnx/tests/adversary_async_batch_end.rs:74:5:
assertion `left == right` failed: input ended, so the async form's None means nothing more at all; the usage record is its last event: []
  left: None
 right: Some("usage")
panicked at adapters/inference-sherpa-onnx/tests/adversary_async_batch_end.rs:95:5:
assertion `left == right` failed: input ended, so iterating's None means nothing more at all; the usage record is its last event: []
  left: None
 right: Some("usage")
test result: FAILED. 0 passed; 2 failed
```
After the control was added and the file formatted, the same two assertions sit at lines 101 and 122.

`adapters/inference-whispercpp/tests/adversary_async_drop_mid_transcription.rs`:
- `a_dropped_recognitions_transcription_does_not_run_beside_the_next_one_admitted`: red now. It counts the threads named `inference-whisp` in `/proc/self/task`.
```
panicked at adapters/inference-whispercpp/tests/adversary_async_drop_mid_transcription.rs:56:5:
a recogniser opened for one recognition at a time runs 2 transcriptions at once after the first was dropped mid-transcription
test result: FAILED. 0 passed; 1 failed
```
(Line 52 after formatting.)

**3. Suite runs, after the cases existed** (`CARGO_INCREMENTAL=0`, `cargo +1.100.0`)
- `cargo test -p inference-sherpa-onnx --features stt --no-fail-fast`: 39 passed. Exit 0. My 3 cases are ignored, so the count does not move.
- `cargo test -p inference-whispercpp --no-fail-fast`: 41 passed. Exit 0. My 1 case is ignored.
- With the checkpoint, `--ignored` on my files plus the unit's `stt_async`:
  - sherpa-onnx: `adversary_async_batch_end` 1 passed, 2 failed; `stt_async` 1 passed. Exit 101.
  - whisper.cpp: `adversary_async_drop_mid_transcription` 1 failed; `stt_async` 1 passed. Exit 101.
- `cargo test -p inference-openai --features duplex,gpt-live --test adversary_async_duplex_reads`: 4 passed. The pass-1 finding is fixed.
- `cargo fmt --all --check`: exit 0. `cargo clippy` on both crates, all targets, `-D warnings`: exit 0.

**4. Findings** (all cover f2ae8019)

1. **sherpa-onnx: input ending while a batch decodes loses the final flush and the usage record.**
   - Where: `adapters/inference-sherpa-onnx/src/stt.rs:922`.
   - Cause: once the in-flight batch is done, `advance` settles it and returns `Ready(())`. It never goes on to the frames or the end of input that arrived meanwhile. A silent batch shows nothing, so `poll_event` returns `Ready(None)` after the end of input — which the contract's new doc says means nothing more at all — and the flush, last commit and usage record never run. Iterating after the same batch ends the same way.
   - The same return also gives a premature `None` mid-input for frames offered while a batch decodes.
   - Who reaches it: an async caller that calls `end_input` while its `event()` is pending, for example a select that ends input when the call's audio stops.
   - Fix (not applied): after settling, loop back and continue instead of returning.
   - NEEDS-CHANGE, introduced.

2. **whisper.cpp: dropping mid-transcription frees the slot while the thread transcribes on.**
   - Where: `adapters/inference-whispercpp/src/stt.rs:590`. `Decoding::start` spawns a thread holding its own `Arc<WhisperContext>`, and nothing stops it when the recognition is dropped.
   - Measured: with `max_concurrency` 1, the next recognition is admitted and two transcriptions run at once.
   - The same shape holds for a sherpa-onnx batch thread (`stt.rs:787`), but its batches are short. I measured only whisper.cpp.
   - Who reaches it: a barge-in that drops the recognition and starts the next turn.
   - Before this commit the iterator could not be dropped mid-transcription.
   - NEEDS-CHANGE, introduced.

3. **The `backend_failure` run through the async form exercises only the trait default.**
   - Where: `crates/inference-testkit/tests/async_handles.rs:529`.
   - The `Failing` recogniser does not override `poll_event`, so the run goes through `Poll::Ready(self.next())` and tests no woken async form. It closes the pass-1 gap by count only.
   - CONFIRMED, introduced.

**5. Attacked and not broken (by reading the code; no case written)**
- OpenAI Realtime and GPT-Live per-poll read: `pending.pop_front` is synchronous, and the stall deadline is anchored to `last_message` across calls, so a read dropped mid-poll loses nothing.
- Their wakeups: the socket's readiness waker survives the dropped read and is woken by the session runtime's thread.
- Their alarm: a past liveness deadline is replaced by now plus the limit, so it does not spin.
- `block` parks the caller's thread rather than entering the runtime, so it does not panic inside a runtime.
- AWS `Arrivals`: the waker is registered before `try_recv`, each sender wakes after sending and when dropped, and the session never wakes itself.
- sherpa-onnx polled while a batch is in flight: it stays `Pending` and the waker is registered before `try_recv`.
- whisper.cpp polled while transcribing: the same, and there is only one batch, at the end of input.

**6. Paths written outside the worktree**
All under `<scratch>/p2/wave3/adversary/`:
- Scripts and messages: `fetch-sherpa.sh`, `run-cases-p2.sh`, `run-suite-p2.sh`, `commit-msg-p2.txt`
- Logs: `fetch-sherpa.log`, `sherpa-case.log`, `whisper-case.log`, `fmt-p2.log`, `suite-sherpa.log`, `suite-whisper.log`, `ignored-sherpa.log`, `ignored-whisper.log`, `openai-pass1-recheck.log`, `clippy-p2.log`
- Done markers: `cases-p2.done`, `suite-p2.done`
- `sherpa-checkpoint/` (254 MB): the pinned checkpoint, all five files matching their pinned hashes: `encoder-epoch-99-avg-1-chunk-16-left-128.onnx`, `decoder-epoch-99-avg-1-chunk-16-left-128.onnx`, `joiner-epoch-99-avg-1-chunk-16-left-128.onnx`, `tokens.txt`, `test_wavs/0.wav`. I left it in place so the sherpa-onnx cases can be re-run.

The whisper.cpp model I used is the existing system file `/usr/share/whisper.cpp-model-base.en/ggml-base.en.bin`; I wrote nothing there.

Commit: 55e56766. `target/` was 1.8 GB, including the sherpa-onnx native archive; it is deleted. I released my worktree lease.

**7. Findings block**
```findings
- file: adapters/inference-sherpa-onnx/src/stt.rs
  line: 922
  category: concurrency
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: when input ends while an async batch decodes, advance settles the batch and returns without decoding the end, so a silent batch makes the async form and the iterator end with None after the end of input and never deliver the usage record (adversary_async_batch_end.rs, two red cases)
- file: adapters/inference-whispercpp/src/stt.rs
  line: 590
  category: concurrency
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a recognition dropped mid-transcription frees its slot while its thread transcribes on, so a recogniser limited to one recognition runs two transcriptions at once (adversary_async_drop_mid_transcription.rs)
- file: crates/inference-testkit/tests/async_handles.rs
  line: 529
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the Failing recogniser keeps the default poll_event, so backend_failure through the async form exercises only Poll::Ready(self.next()) and no woken async form
```