---
format: aep.planning-md/3
id: review-result:local-tts-adapter-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the local TTS adapter
relations:
- reviews: story:local-tts-adapter-under-licence-policy
revision: 1
---
Adversary, pass 1, on story:local-tts-adapter-under-licence-policy, 2026-11-24, as the adversary returned it; `<scratch>` is the pass's scratch directory outside the repository.

unit: story:local-tts-adapter-under-licence-policy
verdict: red
cases: executed 109→119, red 3
origin: introduced 5, pre-existing 0, undecided 0
wrote-outside-worktree: <scratch>/check-realtime-voice.log, <scratch>/check-workspace.log, <scratch>/run1-cases-alone.log, <scratch>/run2-suite-and-cases.log, <scratch>/run3-final-file.log, <scratch>/run4-ignored-without-variable.log
needs-coordinator: yes

Findings cover head 6fd24ef09898f0c4a115729562ffa6df81057d58 plus my one untracked test file. `<scratch>` is the scratch directory the brief assigned. Lease session `w2-u4-adversary-1` is ended.

Needs the coordinator:
- **Finding 1:** the fix is either the doc line or the join; that choice is not mine.
- **Finding 2:** whether `output_streaming` may stand on the letter of the contract.
- **Finding 3:** whether the disclosed delta-by-delta speech is acceptable for the story's goal.
- **Finding 4:** the three live cascade tests need a `llama-server`, which the build node does not have.

## 1. What I touched

`git --no-pager diff --stat` prints nothing (exit 0): no tracked file changed. `git status --short`:

```
?? examples/local-voice-cascade/tests/adversary_w2u4_synthesis.rs
?? examples/local-voice-cascade/tests/security_w2u4_tts_boundary.rs
```

The first file is mine, uncommitted. The second is the second reviewer's; I did not touch it. Nothing was committed and nothing was written under `.engineering/`.

My file sits in the cascade package, not the adapter crate. The adapter has `autotests = false` and one test binary whose module list is `tests/main.rs`, which I may not edit, so a new file there would never compile.

## 2. Cases added

File: `examples/local-voice-cascade/tests/adversary_w2u4_synthesis.rs`, 10 cases. One is unignored and ran on the library without TTS. Nine are ignored, name `INFERENCE_SHERPA_ONNX_TTS_DIR` in their reason, and ran on the library with TTS and the pinned checkpoint under `target/sherpa-onnx-tts` on the build node. They were run three times in total, not in a loop.

| Case | Asserts | Now |
|---|---|---|
| `a_directory_whose_path_holds_a_nul_byte_is_refused_naming_it_and_nothing_panics` (unignored) | `open` and `open_voice` answer `FileMissing` under the directory | green |
| `dropping_a_synthesis_does_not_wait_for_the_sentence_being_generated` | each drop mid-sentence returns within 500 ms | red |
| `the_frames_of_a_one_sentence_segment_start_before_its_generation_ends` | the first frame arrives at least a tenth of the total time before the last | red |
| `a_reply_spoken_delta_by_delta_lasts_about_as_long_as_the_reply_spoken_whole` | a reply as 13 word deltas lasts at most 1.5 times the reply as one segment (the 1.5 is my threshold) | red |
| `a_long_text_without_a_sentence_end_is_spoken_to_its_end` | 678 characters with no sentence end are not truncated | green |
| `no_text_of_letters_or_digits_fails_the_synthesis` | 16 non-English or odd texts each give audio or a failure, never silence | green |
| `every_speaker_of_the_voice_speaks_and_none_outside_it_is_taken` | speakers 0 and 10 speak; -1, 11, `i32::MIN`, `i32::MAX` are refused | green |
| `poll_event_returns_at_once_while_a_sentence_is_generated_and_its_part_wakes_the_task` | the longest poll is under 50 ms and the waker fires before the first frame | green |
| `two_syntheses_generating_at_once_each_speak_their_own_text` | each text is as long as spoken alone, within a tenth | green |
| `a_text_of_100_000_characters_is_stopped_at_its_next_sentence_by_a_discard` | no event of the discarded segment follows; the next segment speaks within 20 s | green |

Red output of the first run of the cases alone, before any suite run (`<scratch>/run1-cases-alone.log`, "Summary [  33.362s] 9 tests run: 6 passed, 3 failed, 1 skipped", exit 100):

```
dropping the synthesiser took 12.818µs, then dropping its synthesis in the middle of a sentence took 5.575726103s
one sentence of 139 characters: 366 frames, 175549 samples; the first frame 6.342198146s after the offer, the last 6.34246083s after it
"It is twenty two degrees outside and it will not rain today." as one segment: 76414 samples, 3183 ms of audio, generated in 2.768088884s; as 13 deltas: 171324 samples, 7138 ms of audio, generated in 8.226776566s; per delta [7249, 10856, 18549, 13581, 20083, 15119, 15687, 7215, 14893, 14799, 16719, 16574, 0]
```

After that run I changed three things in my own file, none of which alters an assertion: an unused variable is now checked, a `while let` became a `for`, and the file was formatted. The same three cases are red on the final file (`<scratch>/run3-final-file.log`, "Summary [  35.716s] 9 tests run: 6 passed, 3 failed, 1 skipped", exit 100):

```
thread 'dropping_a_synthesis_does_not_wait_for_the_sentence_being_generated' (1171674) panicked at examples/local-voice-cascade/tests/adversary_w2u4_synthesis.rs:383:5:
`Drop for SherpaOnnxSynthesis` is documented as "Tells the generation in flight to stop at the engine's next part. It does not wait for it.", but dropping the synthesiser took 15.414µs and dropping the synthesis 6.489528897s: the thread that drops the last of them is held until the sentence is generated to its end

thread 'the_frames_of_a_one_sentence_segment_start_before_its_generation_ends' (1171673) panicked at examples/local-voice-cascade/tests/adversary_w2u4_synthesis.rs:172:5:
the adapter claims output_streaming, partial results before completion: the first frame of a segment of one sentence came 5.862153761s after the offer and the last 5.862327949s after it, so every frame was delivered at once, after the whole segment was generated

"It is twenty two degrees outside and it will not rain today." as one segment: 76468 samples, 3186 ms of audio, generated in 4.936549614s; as 13 deltas: 171213 samples, 7133 ms of audio, generated in 6.935785549s; per delta [7189, 10856, 18550, 13581, 20090, 15105, 15685, 7213, 14892, 14800, 16678, 16574, 0]
thread 'a_reply_spoken_delta_by_delta_lasts_about_as_long_as_the_reply_spoken_whole' (1171669) panicked at examples/local-voice-cascade/tests/adversary_w2u4_synthesis.rs:589:5:
the cascade offers a reply as one segment per delta of the language model: spoken that way it lasts 7133 ms against 3186 ms spoken whole, more than half as long again
```

The audio durations repeat across the three runs (7133 to 7138 ms against 3183 to 3186 ms). The generation times do not; the node is shared.

Tests over 5 s: all nine ignored cases, 16.8 to 35.7 s each in the last run, each including the checkpoint load. None is over 60 s, the step took 35.7 s, and none is in the gate's step.

## 3. Suite run, after the cases existed

`executed <before>` is the implementor's gate count for the two lanes my cases can live in: 81 + 28 = 109. After: 81 + 29 in the gate step, plus 9 ignored cases run by hand on the TTS library, 119.

- **Both sherpa adapters:** `cargo nextest run -p inference-sherpa-onnx-tts -p inference-sherpa-onnx --all-features --no-fail-fast` gave "Summary [   0.521s] 81 tests run: 81 passed, 34 skipped", exit 0 (`<scratch>/run2-suite-and-cases.log`).
- **Cascade, on the final file:** `cargo nextest run -p local-voice-cascade --all-features --no-fail-fast` gave "Summary [   3.093s] 36 tests run: 35 passed, 1 failed, 16 skipped", exit 100 (`<scratch>/run3-final-file.log`).
  - The 36 are the implementor's 28, my 1 unignored case (passed) and 7 cases of the second reviewer's file.
  - The one failure is the second reviewer's `security_w2u4_tts_boundary a_pinned_file_that_is_a_fifo_is_refused_and_is_not_opened`. It is not mine and I did not look into it.
- **Clippy with my file:** `cargo clippy -p local-voice-cascade --all-targets --all-features -- -D warnings`, exit 0. `rustfmt --check` on my file, exit 0.
- **Consumers:** `cargo check -p realtime-voice --all-targets --all-features`, exit 0. `cargo check --workspace --all-targets --all-features`, exit 0.
- **The adapter's ignored tests without the variable:** "Summary [   0.125s] 11 tests run: 0 passed, 11 failed, 41 skipped", exit 100 by design (`<scratch>/run4-ignored-without-variable.log`). Ten panic with "set INFERENCE_SHERPA_ONNX_TTS_DIR to the directory of the unpacked pinned checkpoint"; the eleventh fails at `adapters/inference-sherpa-onnx-tts/tests/open.rs:285` naming the library without TTS.

## 4. Findings

| # | file:line | Verdict | Origin | Severity | Backed by |
|---|---|---|---|---|---|
| 1 | `adapters/inference-sherpa-onnx-tts/src/synthesis.rs:548` | NEEDS-CHANGE | introduced | blocker | red case |
| 2 | `adapters/inference-sherpa-onnx-tts/src/lib.rs:344` | CONFIRMED | introduced | warning | red case |
| 3 | `examples/local-voice-cascade/src/local.rs:612` | CONFIRMED | introduced | warning | red case |
| 4 | `examples/local-voice-cascade/tests/local_adapters.rs:800` | CONFIRMED | introduced | warning | reading, not run |
| 5 | `adapters/inference-sherpa-onnx-tts/tests/record.rs:120` | CONFIRMED | introduced | note | reading, not mutated |

**1. Dropping a synthesis waits for the sentence in flight, and its doc says it does not.**
- Measured: the doc comment at `synthesis.rs:548-549` reads "It does not wait for it." `Workers::drop` at `synthesis.rs:187-194` joins every worker thread. The last holder dropped mid-sentence was held 5.58 s, 3.63 s and 6.49 s in the three runs.
- What reaches it: any caller of the public type that drops the synthesiser and its synthesis while a segment is generated. An async caller using `poll_event` holds its executor thread for that time. In the cascade it is the `tts` node at the end of a run cut while a delta is generated; I did not run that path.

**2. `output_streaming` gives no early frame for a segment of one sentence.**
- Measured: 366 frames of a 139-character sentence arrived within 0.2 ms of each other, 3.8 to 6.3 s after the offer.
- The letter of the contract is met: frames are emitted while the adapter's own record holds the segment Queued, and the crate docs at `lib.rs:41-44` say "one part per sentence". My case asserts the reader's meaning (`website/docs/concepts/contracts.md:42`, "emits partial results before completion"), not the letter.
- The conformance check `claims_observed` (`crates/inference-testkit/src/tts/conformance.rs:671-727`, not the unit's) observes that same Queued state, and its default sentences are single sentences, so it cannot tell the difference.
- What reaches it: the local cascade. Every segment it offers is one language-model delta, never more than a sentence, so no segment streams there, while the `tts` node requires the claim to plan.

**3. A reply spoken delta by delta lasts 2.24 times the reply spoken whole.**
- Measured: 7133 ms against 3186 ms for a 12-word sentence offered as 13 word deltas. Each delta is a separate utterance of 7 000 to 20 000 samples.
- `website/docs/status/limitations.md:58-59` discloses "without the prosody of a sentence" but not the length. The implementor's acceptance run used the mock generator, which answers in whole phrases, so it never spoke this way.
- What reaches it: the documented `talk` example and the guide with any real `llama-server`, where each streamed token is a delta. I split by words; a real server was not available.
- The split into segments is pipeline code (`crates/inference-pipeline/src/nodes/speech.rs:496-507`) that the unit exposes by binding a real synthesiser.

**4. The three live cascade tests now speak through the real synthesiser and nobody has run them.**
- Confirmed by reading: the tests at `local_adapters.rs:800`, `:904` and `:1034` were changed to open the synthesiser; their assertions are unchanged.
- Two risks from reading, neither run:
  - Duration: the answer is capped at 64 tokens (`examples/local-voice-cascade/src/local.rs:124`), each delta gave about 0.55 s of audio in my case, playout is real-time, and the tests wait up to 180 s (`local_adapters.rs:117`) while nextest kills at 60 s.
  - Barge-in: the speech node's `pump` blocks in `synthesis.next()` after each offered delta (`speech.rs:423`, `:507`), so a supersede queued behind committed deltas is read only after each has been generated. The test at `:1034` is exactly that case.
- What reaches it: whoever runs the ignored live tests as the guide describes.

**5. The resource-profile test passes with any positive numbers.**
- Read at `record.rs:118-127`: memory, load time and real-time factor are each asserted `> 0`. The STT precedent bounds the factor to `1..1_000` (`adapters/inference-sherpa-onnx/tests/stt_record.rs:105-106`).
- A record from another machine is caught only by the hardware string at `record.rs:69`.
- What reaches it: an edit of `records/tts-xeon-8375c-cpu.json`.

## 5. Attacked and could not break

- **NUL byte in a directory path:** refused as `FileMissing` by `open` and `open_voice`, no panic (library without TTS).
- **Non-English and odd texts:** all 16 gave audio with no failure, none silent.
- **Long text without a sentence end:** 678 characters gave 6.78 times the audio of the 96-character text, in 2 bursts.
- **Speaker ids:** 0 and 10 speak; -1, 11, `i32::MIN` and `i32::MAX` are refused.
- **`poll_event` during generation:** the longest poll was 44.4, 20.4 and 14.6 ms over the three runs, under the 50 ms ticker bound; the first was close, on an unoptimised build. The waker fired once before the first frame.
- **Two syntheses at once (concurrency 2):** each spoke its own text, within a few samples of spoken alone.
- **100 035 characters, then discard:** no event of the discarded segment followed; the next segment's first frame came 2.9 to 4.2 s after the discard; both handles dropped in 47 to 76 ms.
- **Consumers:** every caller of `declare_local`, `local_nodes`, `local::synthesiser` and `LocalModels` compiles. `examples/realtime-voice/examples/compare.rs:450-504` keeps its meaning.
- **Earlier adversary file `adversary1_stt_swap.rs`:** the implementor's 7-line edit only follows the renamed function.
- **`rg -n "waits on the licence policy"`** prints nothing outside `.engineering/`.
- **Cargo target runner line** of the README and the guide: I used that form for every run on the TTS library and it selected the library.
- **`errors.md` link for `NonPositiveConcurrency`:** true by the page's own definition, since the new crate's `lib.rs` does name the kind.
- **Guide's pasted output** (`run-the-local-voice-cascade-offline.md:193`): it differs from the implementor's final run by a few samples; sample counts vary by a few per run in my cases too.
- **Test budget:** the 81 unignored tests of both adapters ran in 0.5 s.

Not attacked:
- A 22 050 Hz voice on the real library: there is no Piper voice on the node and downloads are forbidden. Only the unit's scripted-engine test covers it.
- Empty and whitespace-only text: read at `synthesis.rs:509-518` (completes without audio), not run by me.
- Freeing of the native handle and the licence boundary: the second reviewer's part.

## 6. Paths written outside the worktree

- `<scratch>/check-realtime-voice.log`
- `<scratch>/check-workspace.log`
- `<scratch>/run1-cases-alone.log`
- `<scratch>/run2-suite-and-cases.log`
- `<scratch>/run3-final-file.log`
- `<scratch>/run4-ignored-without-variable.log`
- Build output under `target/` of this worktree's path on the build node (inside the worktree there, ignored by git).
- The harness's own task-output file for one background command, outside my scratch; its output was redirected to `<scratch>`, so it should be empty.

## 7. Findings block

```findings
- file: adapters/inference-sherpa-onnx-tts/src/synthesis.rs
  line: 548
  category: contract-drift
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the doc of Drop for SherpaOnnxSynthesis says it does not wait for the generation in flight, but Workers::drop at :187-194 joins the worker and the dropping thread was held 3.6 to 6.5 s mid-sentence
- file: adapters/inference-sherpa-onnx-tts/src/lib.rs
  line: 344
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: output_streaming is claimed, yet all 366 frames of a one-sentence segment arrive within 0.2 ms after the whole segment is generated, and every segment the local cascade offers is such a segment
- file: examples/local-voice-cascade/src/local.rs
  line: 612
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: with the real synthesiser bound, a 12-word reply offered as 13 deltas lasts 7133 ms against 3186 ms offered whole, which the limitations page does not say and the acceptance run with whole-phrase mock answers never exercised
- file: examples/local-voice-cascade/tests/local_adapters.rs
  line: 800
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the three live cascade tests at :800, :904 and :1034 now speak through the real synthesiser and were run by nobody, while reading shows a risk against the 60 s kill and a supersede waiting behind the speech node's blocking pump
- file: adapters/inference-sherpa-onnx-tts/tests/record.rs
  line: 120
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the resource-profile test asserts only that memory, load time and real-time factor are positive, so it passes with any changed numbers, where the STT precedent bounds the factor to 1..1000
```
