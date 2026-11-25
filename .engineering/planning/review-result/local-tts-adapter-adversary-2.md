---
format: aep.planning-md/3
id: review-result:local-tts-adapter-adversary-2
kind: review-result
status: active
title: Adversary pass 2 on the local TTS adapter
relations:
- reviews: story:local-tts-adapter-under-licence-policy
revision: 1
---
Adversary, pass 2 (the last), on story:local-tts-adapter-under-licence-policy, 2026-11-25, as the adversary returned it; `<scratch>` is the pass's scratch directory outside the repository.

unit: story:local-tts-adapter-under-licence-policy
verdict: red
cases: executed 183→192, red 3
origin: introduced 6, pre-existing 0, undecided 0
wrote-outside-worktree: `<scratch>/` (54 files and 2 directories, listed in part 6), 5 task-output files of the harness, `target/` on the build node
needs-coordinator: yes

Pass 2 of the adversary on the correction `6fd24ef09..cc965b0af`, branch `impl/local-tts-adapter`.
The findings cover the working tree at `cc965b0afa60d60224d90bb9a7e7361c27401ca7` plus the three
uncommitted paths of part 1. Nothing is committed. No implementation file, document or existing
test was edited. The lease session `w2-u4-adversary-2` is ended.

Needs the coordinator, four points:

1. Finding 1 is a `warning` with a red case: decide whether it goes back to the implementor.
2. My case `a_regular_file_far_larger_than_the_pinned_one_...` is **not ignored** and red (finding 5,
   `INFEASIBLE`). If my files are committed as they are, the gate lane of
   `inference-sherpa-onnx-tts` is red. Remove the case or mark it, as you decide.
3. **`<scratch>/report.md` does not exist.** Writing it was refused, with this text: "Subagents
   should return findings as text, not write report files. Include this content in your final
   response instead." I did not try another way. This message is the only copy of the report.
4. A deviation I decided myself: the build node's mirror of the worktree was new and held neither
   native library nor the checkpoint, against the brief's premise. I ran the repository's own two
   tasks, `task sherpa-onnx-archive` and `task sherpa-onnx-tts`, once each on the node (both check
   the sha256 of what they fetch). I read this as inside "Download nothing else": it restored the
   two things the brief names. Logs: `<scratch>/fetch-*.log`.

One more decision of mine: the brief allows planting a defect in the tree and reverting it; the
role forbids mutating a file under attack. I took the stricter rule. The licence edges were planted
only in a copy (`git archive HEAD | tar -x` into `<scratch>`, removed again). So the licence test
itself was not run red; its result below rests on what `cargo metadata` prints for each planted
copy and on reading `tests/licence.rs:259-285`.

## 1. `git --no-pager diff --stat`

```
 adapters/inference-sherpa-onnx-tts/tests/main.rs | 1 +
 1 file changed, 1 insertion(+)
```

`git status --short`:

```
 M adapters/inference-sherpa-onnx-tts/tests/main.rs
?? adapters/inference-sherpa-onnx-tts/tests/adversary2_w2u4_files.rs
?? examples/local-voice-cascade/tests/adversary2_w2u4_synthesis.rs
```

The one changed line is `mod adversary2_w2u4_files;`. All three paths are test paths.

## 2. The cases added

Nine cases, 3 red and 6 green now.

### `adapters/inference-sherpa-onnx-tts/tests/adversary2_w2u4_files.rs` (5 cases, not ignored, no weights)

| Case | Asserts | Now | Time |
|---|---|---|---|
| `a_pinned_file_that_is_a_link_to_a_fifo_or_to_a_device_is_refused_without_being_opened` | `open` answers `FileMissing { kind: NOT_REGULAR }` within 3 s for a model that is a link to a FIFO and to `/dev/zero` | green | 0.007 s |
| `a_link_that_leads_back_to_itself_is_refused_by_both_constructors` | `open` and `open_voice` refuse a model that is a link to itself | green | 0.006 s |
| `a_tree_that_links_back_into_itself_is_refused_and_its_walk_ends` | `tree_sha256` returns an error within 3 s for a tree with links to its own directories | green | 0.008 s |
| `a_pinned_file_that_may_not_be_read_is_refused_as_not_readable` | `open` answers `FileMissing { kind: PermissionDenied }` for a model of mode 000 | green | 0.006 s |
| `a_regular_file_far_larger_than_the_pinned_one_is_refused_without_being_read_to_its_end` | `open` answers within 3 s when `model.onnx` is a sparse regular file of 64 GiB | **red** | 3.0 s |

Run alone, before any suite run (`<scratch>/run1-adapter.log`), command
`cargo nextest run -p inference-sherpa-onnx-tts --all-features --no-fail-fast -E 'test(/^adversary2_w2u4_files::/)'`,
exit 100:

```
        PASS [   0.006s] (1/5) inference-sherpa-onnx-tts::main adversary2_w2u4_files::a_link_that_leads_back_to_itself_is_refused_by_both_constructors
        PASS [   0.006s] (2/5) inference-sherpa-onnx-tts::main adversary2_w2u4_files::a_pinned_file_that_may_not_be_read_is_refused_as_not_readable
        PASS [   0.007s] (3/5) inference-sherpa-onnx-tts::main adversary2_w2u4_files::a_pinned_file_that_is_a_link_to_a_fifo_or_to_a_device_is_refused_without_being_opened
        PASS [   0.008s] (4/5) inference-sherpa-onnx-tts::main adversary2_w2u4_files::a_tree_that_links_back_into_itself_is_refused_and_its_walk_ends
        FAIL [   3.008s] (5/5) inference-sherpa-onnx-tts::main adversary2_w2u4_files::a_regular_file_far_larger_than_the_pinned_one_is_refused_without_being_read_to_its_end
  stderr ───

    thread 'adversary2_w2u4_files::a_regular_file_far_larger_than_the_pinned_one_is_refused_without_being_read_to_its_end' (1422567) panicked at adapters/inference-sherpa-onnx-tts/tests/adversary2_w2u4_files.rs:188:5:
    open did not answer within 3s for a directory whose model.onnx is a regular file of 68719476736 bytes: no length is pinned, so a file of any length is read to its end before its sha256 is compared
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

     Summary [   3.009s] 5 tests run: 4 passed, 1 failed, 58 skipped
```

The 3 s threshold is this pass's, not a documented one. The case creates no data block and removes
the file before it asserts.

### `examples/local-voice-cascade/tests/adversary2_w2u4_synthesis.rs` (4 cases, all ignored, need the checkpoint and the library with text-to-speech)

Each ignore reason names `INFERENCE_SHERPA_ONNX_TTS_DIR`; run without it, each fails naming it.

| Case | Asserts | Now | Time alone / in a lane |
|---|---|---|---|
| `a_segment_of_one_long_sentence_is_delivered_once_as_the_pages_say` | one sentence of 679 characters arrives as one part: `(last - first) * 10 < last`, the assertion of the unit's own case at 139 characters | **red** | 21.5 s / 35.4 s / 41.4 s |
| `what_is_started_beside_the_thread_of_a_dropped_synthesis_speaks_its_own_text_whole` | after a drop mid-sentence, the same synthesiser and a second one each speak a short reply within 10 % of its samples alone; each drop under 500 ms | green | 20.4 s / 27.2 s / 35.3 s |
| `a_process_that_ends_as_the_thread_of_a_dropped_synthesis_ends_exits_with_status_0` | six child processes return from `main` -60 to +150 ms around the moment the left-behind thread is due to end; each exits 0 | green | 22.4 s / 34.2 s / 39.7 s |
| `a_voice_model_that_may_not_be_read_is_refused_and_the_process_that_opens_it_lives` | a child that calls `open_voice` on a voice whose model has mode 000 gets `Err(` and exits 0 | **red** | 2.9 s to 4.6 s |

Three of them run over 5 s; the times are in the table. The longest seen is 41.4 s, with four
cases at once.

First run alone, before any suite run (`<scratch>/run1-cascade.log`), command
`cargo nextest run -p local-voice-cascade --all-features --run-ignored only --no-fail-fast -E 'binary(adversary2_w2u4_synthesis)'`
with the runner and the variable of the ignored lane, exit 100:

```
        FAIL [  21.520s] (3/3) local-voice-cascade::adversary2_w2u4_synthesis a_segment_of_one_long_sentence_is_delivered_once_as_the_pages_say
  stderr ───
    one sentence of 679 characters: 2039 frames, 978380 samples; the first frame 9.049926835s after the offer, the last 13.405064549s after it; waits of over 100 ms between two frames, as (frames before, wait): [(1338, 4.354484266s)]

    thread 'a_segment_of_one_long_sentence_is_delivered_once_as_the_pages_say' (1427341) panicked at examples/local-voice-cascade/tests/adversary2_w2u4_synthesis.rs:155:5:
    the pages say a segment of one sentence is one part, delivered once after it is generated, but the first frame of a sentence of 679 characters came 9.049926835s after the offer and its last 13.405064549s after it, in 2 parts
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

The case on a file that may not be read was changed twice after that first run, each time before
any suite run. The first form tried the model and the tokens. Its tokens half proved nothing: the
only model at hand is the Kokoro one, and the child ended with status 255 at the model's family
before its tokens counted (that run's log was overwritten by the next). I withdrew that half and
renamed the case. **Whether a voice whose tokens may not be read is refused is not shown by this
pass.** The final form, alone (`<scratch>/run1b-unreadable.log`), exit 100:

```
        FAIL [   4.005s] (1/1) local-voice-cascade::adversary2_w2u4_synthesis a_voice_model_that_may_not_be_read_is_refused_and_the_process_that_opens_it_lives
  stderr ───
    the model may not be read: the child ended with signal: 6 (SIGABRT) (core dumped); it wrote None

    thread 'a_voice_model_that_may_not_be_read_is_refused_and_the_process_that_opens_it_lives' (1642910) panicked at examples/local-voice-cascade/tests/adversary2_w2u4_synthesis.rs:295:5:
    a voice whose model may not be read is refused and the process that opens it lives, but the child ended with signal: 6 (SIGABRT) (core dumped) and wrote:
    fatal runtime error: Rust cannot catch foreign exceptions, aborting
```

The model in that case is 11 bytes that are not a model. The case first asserts that its own
process cannot open the file, so the abort follows from the permission and not from the bytes.
(The panic line `:295` is of the file before the fourth case was added above it; the assertion is
now at `:381`.)

The case on the end of a process was written after the first suite run and run alone twice
(`<scratch>/run1c-process-end-1.log`, `-2.log`), exit 0 both times. First run:

```
offset -60 ms: the child ended with exit status: 0; Some("the child returns 5.010084044s after the offer; the sentence took 5.070028381s before")
offset -20 ms: the child ended with exit status: 0; Some("the child returns 4.789078612s after the offer; the sentence took 4.809989469s before")
offset 0 ms: the child ended with exit status: 0; Some("the child returns 4.696086186s after the offer; the sentence took 4.696350845s before")
offset 20 ms: the child ended with exit status: 0; Some("the child returns 4.982090587s after the offer; the sentence took 4.962666993s before")
offset 60 ms: the child ended with exit status: 0; Some("the child returns 5.048652944s after the offer; the sentence took 4.988119606s before")
offset 150 ms: the child ended with exit status: 0; Some("the child returns 5.071077117s after the offer; the sentence took 4.921698723s before")
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 22.43s
```

## 3. The suite run, after the cases existed

Script `<scratch>/run2-suite.sh`, summary `<scratch>/run2-summary.log`. Each lane is one `cargo
nextest run` through the build pool: the lanes of the crates the unit touches, not the workspace.

| Lane | Command (filter) | Result | Exit | Wall |
|---|---|---|---|---|
| A, gate | `-p inference-sherpa-onnx-tts -p inference-sherpa-onnx --all-features` | 91 run, 90 passed, 1 failed (my sparse case), 35 skipped | 100 | 4.0 s |
| B, gate | `-p local-voice-cascade --all-features` | 37 run, 37 passed, 20 skipped | 0 | 7.2 s |
| C, ignored, library with TTS | `-p inference-sherpa-onnx-tts --run-ignored only -E 'not test(without_tts) and not test(one_short_utterance)'` | 10 run, 10 passed | 0 | 47.5 s |
| D, ignored, library with TTS | `-p local-voice-cascade --run-ignored only`, the three synthesis files and the two cases of `local_adapters` that need no server | 16 run, 14 passed, 2 failed (my two red cases) | 100 | 232.3 s |
| E, ignored, no variable | both packages, the same cases | 28 run, 28 failed | 100 | 0.1 s |
| F, docs | `-p inference-docs` | 36 run, 35 passed, 1 failed: `every_binding_is_since_the_release_whose_changelog_section_introduced_it`, the settled one | 100 | 0.3 s |
| G | `cargo run -q -p inference-docs -- generate --check` | every page "current" | 0 | |
| H | `cargo clippy -p inference-sherpa-onnx-tts -p local-voice-cascade --all-targets --all-features -- -D warnings` | no warning | 0 | |

Afterwards: the cascade file in its final state of four cases (`<scratch>/run4-summary.log`: 4 run,
2 passed, 2 failed, 41.4 s; clippy again exit 0), and the one ignored case of the library without
TTS with the variable set (`<scratch>/run5-without-tts.log`: passed, 8.2 s).

Counts: the gate step ran 159 before (implementor's count) and 164 now (91 + 37 + 36). The ignored
lane ran 24 before and 28 now (10 + 16 + 1 + my fourth case). Together 183→192. The three live
cascade tests and the measurement `one_short_utterance` were not run.

Lane D ran one test at a time in its pool job, so its wall time is the sum of its tests: 232 s, of
which my three cases of that run are 67 s. No test ran over 60 s; the longest was 35.4 s. It is the
ignored lane, not the gate's test step. The gate lanes A, B and F together ran under 12 s.

No unignored test of the two crates ran over 2 s except my sparse case at 3.0 s.

Lane E: 25 of the 28 fail naming `INFERENCE_SHERPA_ONNX_TTS_DIR`. Three check the library first and
fail naming the library without TTS instead: the two cases of `security_w2u4_tts_boundary.rs` with
a sentence, and `open::the_pinned_checkpoint_opens_and_its_phoneme_data_is_the_pinned_tree`
(`tests/open.rs:292`) with the bare `Absent { library: … }`. By reading, each of the three reaches
the variable only after the library check. Not a finding.

## 4. Findings

All cover the tree named at the top. All six are `introduced`: the crate does not exist at the base
`6a5c68ec0`, and the sentences of findings 2, 3, 4 and 6 are added lines of the correction.

| # | file:line | Verdict | Origin | Severity | What was measured | What reaches it |
|---|---|---|---|---|---|---|
| 1 | `adapters/inference-sherpa-onnx-tts/src/lib.rs:848` | CONFIRMED | introduced | warning | `open_voice` checks its model with `filled` (`:586`), by metadata alone. A model file of at least one byte that the process may not read passes it, sherpa-onnx is called, and the process aborts with `SIGABRT`. `OpenRefused::FileMissing` is documented at `:404` for a file that "is not readable". The section on voices that end the process names a file that is not a model and a model of another family, not this. Red case: `a_voice_model_that_may_not_be_read_...`. | The public constructor `open_voice` / `open_voice_with`, documented in the crate's README at `:69`. No caller in this workspace. The state is a voice file with the wrong owner or mode, no hostile input. The tokens file is not shown either way. |
| 2 | `adapters/inference-sherpa-onnx-tts/README.md:94` | CONFIRMED | introduced | note | "A segment of one sentence is one part: its frames are delivered together once the sentence is generated." One sentence of 679 characters came in 2 parts, 4.35 s apart (5.74 s in a second run). The same sentence is in `src/lib.rs:54`, `website/docs/status/limitations.md:58`, `crates/inference-docs/src/adapters.rs:249` and so `website/docs/reference/adapters.md:36`. The unit's case holds it at 139 characters only. Red case: `a_segment_of_one_long_sentence_...`. | A caller of the adapter that offers one long sentence as one segment. Where between 139 and 679 characters sherpa-onnx starts to split was not located. The local cascade does not reach it: its segments are single deltas. |
| 3 | `examples/local-voice-cascade/tests/local_adapters.rs:132` | CONFIRMED | introduced | note | By reading, not by a run. The arithmetic for the longest live case leaves out what the same correction writes on the limitations page: every delta of the first answer committed before the supersede is generated to its end before the supersede is read. With a server faster than the synthesiser that is up to about 19 deltas of about 0.55 s each. "Generation is faster than playout" rests on the record's real-time factor 0.553, while word deltas were measured in pass 1 at about 1.0 (7.13 s of audio generated in 6.94 to 8.23 s). The stated sums are correct as sums (39.0 s, 21.0 s, 2.3 tokens a second). With the missing term I get about 43 to 53 s for the longest case: still under 60 s, with 7 to 17 s to spare where the comment says 21. | Whoever runs the three ignored live cases, and whoever takes "2.3 tokens a second" from the comment as the server's lower bound. |
| 4 | `examples/local-voice-cascade/tests/adversary_w2u4_synthesis.rs:593` | CONFIRMED | introduced | note | By reading. The comment of `known_limit_...` says "when this case turns red, the limit is gone". The case offers 13 segments straight to the adapter and compares sample counts. The draft story changes what the cascade offers, not the adapter, so the case stays green when the limit is gone. It guards the adapter's ratio and the pages' two numbers, not the limit. | A reader who waits for this case to announce that the pages need rewriting. |
| 5 | `adapters/inference-sherpa-onnx-tts/src/lib.rs:517` | INFEASIBLE | introduced | note | `NOT_REGULAR` is documented with "reading one can block without end, so it is never opened". A regular file of any length is hashed to its end before its sha256 is compared: `CheckpointFile` pins no length. By reading, the tree walk (`:548`) likewise follows a link to a directory outside the tree and hashes all of it before the file count is compared. Red case: the sparse file of 64 GiB. | Nothing found. I built the state; it needs someone who writes into the checkpoint directory. The 3 s threshold is mine. |
| 6 | `adapters/inference-sherpa-onnx-tts/src/tests.rs:464` | INFEASIBLE | introduced | note | By reading. The unit test `dropping_the_last_handle_does_not_wait_for_the_part_being_generated` allows 50 ms from before a `thread::spawn` to that thread's send. It is not ignored and runs in the gate on a shared node. It passed in every run here. | Nothing shown: I did not see it fail and did not load the node to make it fail. |

## 5. Attacked and not broken

- The drop that no longer joins: a synthesis started on the same synthesiser while the thread of a dropped one still generates speaks its own text whole, and so does a second synthesiser after the first's last handle is gone (my green case; each drop under 500 ms).
- Process end: 3 more runs of the unit's `process_end` case (status 0 each) and 12 child processes of my case returning around the thread's due end (status 0 each). The claim in `src/synthesis.rs:582` and `README.md:103` stays a sampled one: whether any child landed inside the engine's release is not known, and no public call waits for the thread.
- `Workers` by reading: the count after a panic in the worker, `wait_idle` with no worker, the engine freed once, a drop inside any runtime, frames kept per synthesis.
- Files by metadata: a link to a FIFO, a link to a device, a link to itself (both constructors), a tree that links into itself, a pinned file of mode 000: all refused, each within 10 ms (4 green cases).
- The licence test: `cargo metadata --format-version 1 --no-deps --offline` exits 0 with an empty `CARGO_HOME`. In a copy, a dev-dependency in table form, a build-dependency, and a renamed, optional, target-gated dependency each show as a second dependent, which `tests/licence.rs:259-285` refuses. A new crate that depends on `local-voice-cascade` with its feature `local` is not seen by that test; it is seen by `only_the_offline_cascade_names_the_adapter_and_only_behind_its_feature` in `security_w2u4_tts_boundary.rs`, so only that second file holds the transitive form.
- The record test's bounds (`tests/record.rs:117-145`): the record's 483 MiB, 8050 ms and 553 lie inside; the test passes for any number inside the bounds, as ruled.
- The two rewritten security cases (`security_w2u4_tts_boundary.rs:215`, `:244`): neither can pass with a child that lives; both green in lane D.
- The rewritten case `a_one_sentence_segment_is_delivered_once_and_two_sentences_part_by_part`: green; its one-sentence half does not hold for a long sentence (finding 2).
- Pages and numbers: the archive URL and sha256 in the README equal `Taskfile.yml`; 7133 / 3186 ms, 399 frames, 2275 / 5671 ms, 760 / 1072 ms and "within 3 ms" match the logs of the correction and of pass 1.
- `generate --check`: exit 0, every page current.
- Test budget: gate lanes of the touched crates under 12 s together; no test over 60 s in any lane.
- Ignored tests and their variable: every ignore reason of the two crates names its variable; lane E above.
- Not attacked by a run: the three live cascade tests (by the brief), the tokens file of a caller's voice, and `open_voice` with a real VITS voice (none at hand).

## 6. Paths written outside the worktree

In `<scratch>/` (the directory `wave2/u4/adversary-2` of the session's scratch directory):

- `fetch.sh`, `fetch-summary.log`, `fetch-no-tts.log`, `fetch-tts.log`, `fetch-ls.log`
- `run0-no-library-summary.log`, `run0-no-library-adapter.log`, `run0-no-library-cascade.log` (the first attempt, which failed in the build script for the missing library; no case ran)
- `run1-cases-alone.sh`, `run1-summary.log`, `run1-adapter.log`, `run1-cascade.log`
- `run1b-unreadable-alone.sh`, `run1b-summary.log`, `run1b-unreadable.log`
- `run1c-process-end-alone.sh`, `run1c-summary-1.log`, `run1c-summary-2.log`, `run1c-process-end-1.log`, `run1c-process-end-2.log`
- `run2-suite.sh`, `run2-summary.log`, `run2-a-adapters.log`, `run2-b-cascade.log`, `run2-c-adapter-ignored.log`, `run2-d-cascade-ignored.log`, `run2-e-no-variable.log`, `run2-f-docs.log`, `run2-g-generate-check.log`, `run2-h-clippy.log`
- `run3-process-end.sh`, `run3-summary.log`, `run3-process-end-1.log`, `run3-process-end-2.log`
- `run4-final.sh`, `run4-summary.log`, `run4-cascade-file.log`, `run4-clippy.log`
- `run5-without-tts.sh`, `run5-without-tts.log`
- `licence/`: `plant.sh`, `plant.log`, `metadata-empty-home.json`, `metadata-empty-home.err`, and `meta-<plant>.json` with `meta-<plant>.err` for the five plants `unchanged`, `dev-dependency-table-form`, `build-dependency`, `renamed-optional-target-gated`, `through-the-cascade-with-its-feature`
- `empty-cargo-home/` (empty)

`<scratch>/report.md` was not written (refused, see the top).

Elsewhere:

- Five task-output files of the harness, one per background command, in the session's task directory.
- On the build node, in its mirror of this worktree: build output under `target/`, the two fetched archives and what they unpack to under `target/sherpa-onnx/` and `target/sherpa-onnx-tts/` (39 MB and 699 MB), and the test directories under `target/tmp/`.
- The child of the red case on the unreadable model ended with "core dumped", as the child of the unit's own case at `security_w2u4_tts_boundary.rs:215` does. Whether the node kept a core file I did not check.

## 7. Findings block

```findings
- file: adapters/inference-sherpa-onnx-tts/src/lib.rs
  line: 848
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: open_voice checks its model by metadata alone, so a model file the process may not read reaches sherpa-onnx and aborts the process, where OpenRefused::FileMissing is documented for a file that is not readable
- file: adapters/inference-sherpa-onnx-tts/README.md
  line: 94
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the sentence that a segment of one sentence is one part delivered together is false for a sentence of 679 characters, which arrives in two parts seconds apart, and the same sentence stands in the crate documentation, the limitations page and the adapters reference
- file: examples/local-voice-cascade/tests/local_adapters.rs
  line: 132
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the arithmetic for the live cases leaves out the deltas of the superseded answer that are generated before the supersede is read and takes generation as faster than playout for word deltas, so the margin under 60 s is smaller than the 21 s it states
- file: examples/local-voice-cascade/tests/adversary_w2u4_synthesis.rs
  line: 593
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the comment says the known-limit case turns red when the limit is gone, but the case offers its segments straight to the adapter and stays green when the cascade offers whole sentences
- file: adapters/inference-sherpa-onnx-tts/src/lib.rs
  line: 517
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a regular file of any length in the place of a pinned file is hashed to its end before its sha256 is compared because no length is pinned, a state only someone writing into the checkpoint directory builds
- file: adapters/inference-sherpa-onnx-tts/src/tests.rs
  line: 464
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the unit test on the drop that does not wait allows 50 ms across a thread spawn on a shared build node, a bound not seen to fail in this pass
```
