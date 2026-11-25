---
format: aep.planning-md/3
id: review-result:local-tts-adapter-security-1
kind: review-result
status: active
title: Security review of the local TTS adapter
relations:
- reviews: story:local-tts-adapter-under-licence-policy
revision: 1
---
Security review, pass 1, on story:local-tts-adapter-under-licence-policy, 2026-11-24, as the reviewer returned it; `<scratch>` is the pass's scratch directory outside the repository.

unit: story:local-tts-adapter-under-licence-policy
verdict: red
cases: executed 145→152, red 2 in the gate lanes (1 added by this pass, 1 the implementor's known docs case); ignored lane on the library with TTS: 2 added, 2 red
origin: introduced 8, pre-existing 1, undecided 0
wrote-outside-worktree: `<scratch>/` only (22 files, listed in part 6); `<scratch>` is `wave2/u4/security-1`
needs-coordinator: yes

Findings cover head 6fd24ef09898f0c4a115729562ffa6df81057d58 plus one untracked file of mine. Strongest verdict in the role's terms: NEEDS-CHANGE.

## 1. What I touched

`git --no-pager diff --stat`: empty. `git status --short`:

```
?? examples/local-voice-cascade/tests/adversary_w2u4_synthesis.rs     (the adversary's, not touched)
?? examples/local-voice-cascade/tests/security_w2u4_tts_boundary.rs   (mine, uncommitted)
```

- No implementation file and no existing test was edited.
- My cases are in the cascade's `tests/`, not the adapter's: `adapters/inference-sherpa-onnx-tts/Cargo.toml` sets `autotests = false`, so a new file there is not built without a manifest or `tests/main.rs` edit.

## 2. The invariants

| # | Invariant | Enforced by |
|---|---|---|
| 1a | No GPL file in the tree or the package | `nothing_of_espeak_ng_or_of_the_native_library_is_in_the_repository` (`adapters/inference-sherpa-onnx-tts/tests/licence.rs:175`); it walks the working tree and skips hidden directories. I checked `git ls-files` and `cargo package --list -p inference-sherpa-onnx-tts --allow-dirty --offline` (18 files): none. |
| 1b | Every workspace build links the archive without TTS | `.cargo/config.toml:2` (`force = true`, unchanged) and `sherpa-onnx-sys` `build.rs:117-126`, which fails on a missing directory instead of downloading. Tests: `no_gpl_phonemiser_is_linked` (unchanged, green, 81 of 81 in its lane); the new crate's `the_library_the_workspace_builds_against_is_read_as_built_without_tts` (`tests/open.rs:240`, a marker string, not a symbol scan). The cascade with `local` had none: added `the_cascade_with_the_synthesiser_bound_maps_no_gpl_phonemiser_in_the_gate`, green. |
| 1c | Only the cascade depends on the adapter, behind `local` | The existing test reads one manifest form only (finding 3). `cargo metadata`: `local-voice-cascade` is the one declared dependent (`optional=true`); `realtime-voice` reaches it through its feature `local`; no node has it under default features. Added a strict case, green. |
| 1d | Builds that run without `SHERPA_ONNX_LIB_DIR` | None in `.github/workflows/` (`check.yml:137-145`, `pages.yml:52,56`, `slow.yml:93` all run from the root) and none in `Taskfile.yml`. Outside the workspace: `adapters/inference-sherpa-onnx-tts/README.md:97-99` and a Git dependency as `website/docs/getting-started.md:58-66` shows (findings 6 and 7). |
| 1e | Licence texts agree with the approval record | They agree on its two points. Differences: findings 8 and 9. The record's status is `draft`, as the implementor reported. |
| 2 | Fetch task | Was convention only. `Taskfile.yml:94-121`: fixed `https` URLs; each hash is checked on the `.part` file before the move and before `tar`; a mismatch stops the task with nothing unpacked; `curl --fail` or the hash catches a partial download; `--location` follows redirects and the bytes are still hash-checked. Now held by two added cases, green. Not established: the archives' entries (`..`, absolute paths, symlinks); they are on the build node and I did not list them, so path safety rests on `tar`'s defaults and the pinned bytes. |
| 3 | Native boundary | The crate has no `unsafe` (the workspace forbids it); all FFI is `sherpa-onnx` 1.13.8. Text NUL: refused twice (`src/synthesis.rs:505`, `src/engine.rs:23`), test at `src/tests.rs:173`. Path NUL: refused by std before the native call; added case, green. Callback: boxed and freed by upstream around the call (`tts.rs:598-620`), runs only inside the native call on a worker that holds its own `Arc` of the engine; a panic in it aborts the process (`extern "C"` trampoline, `tts.rs:638`), and the only foreign code in it is the caller's waker. Samples: null and `n <= 0` guarded upstream (`tts.rs:645`). `Send`/`Sync`: inherited from upstream's `unsafe impl` (`tts.rs:524-526`); this crate writes no reason of its own. Not a safe boundary for bad files: finding 2. |
| 4 | Files from a caller | Findings 1, 2, 4, 5. No JSON is read: a Piper `.onnx.json` is ignored. Files are hashed as a stream. Validated in Rust before the native read: for the pin, three sha256 and the tree digest; for a caller's voice, only that tokens and the data directory exist, that there is exactly one `*.onnx`, and that paths are Unicode. |
| 5 | No network in tests or default paths | By reading every `rg` hit: URLs are constants and documentation only. No test or other task runs the fetch task: added case, green. |
| 6 | Nothing private in the diff | No host name, user name or home path in the diff or the five commit messages (all by the bot identity). The record and one page say "build node" and name a Xeon Platinum 8375C; whether that is a "host detail" under `AGENTS.md` is yours. |

## 3. Findings

| # | file:line | What was measured | What reaches it | Verdict | Origin |
|---|---|---|---|---|---|
| 1 | `adapters/inference-sherpa-onnx-tts/src/lib.rs:537` | `open` on a directory whose `model.onnx` is a FIFO does not answer within 3 s. `present` accepts anything that is not a directory, then `sha256_of` (`:491`) opens it. The docs promise `FileMissing` for a path "not of that kind". Red case below. | `SherpaOnnxSynthesiser::open(dir)`, `talk --tts-dir`, `INFERENCE_SHERPA_ONNX_TTS_DIR`, on any directory that is not the task's own unpack. A symlink to a device takes the same path and never ends; the tree walk (`:519-522`) hashes any non-directory too. | NEEDS-CHANGE | introduced |
| 2 | `adapters/inference-sherpa-onnx-tts/src/lib.rs:571` | `open_voice` ends the process instead of returning `BackendRefused`: SIGABRT for a model file that is not a model, exit 255 for a loadable model of another family. The docs at `:748-749` and `tests/open.rs:1-3` ("none panics") say otherwise. Two red cases below. | `open_voice(dir)`, the consumer's stated need ("a voice directory as configuration"): a download cut short, or a Piper voice not converted for sherpa-onnx. The pinned `open` is not reached: its files are hash-checked first. | NEEDS-CHANGE | introduced |
| 3 | `adapters/inference-sherpa-onnx-tts/tests/licence.rs:214` | The dependents check reads a manifest only when a line starts with `inference-sherpa-onnx-tts ` (with the space). The same rule as a `grep` over six hand-built manifests reads the inline form and misses the table, renamed, inherited, no-space and quoted forms (`<scratch>/predicate-probe.sh`). | Nothing found: no manifest uses those forms for this crate today. The inherited form would still go red through the root entry. | INFEASIBLE | introduced |
| 4 | `adapters/inference-sherpa-onnx-tts/src/lib.rs:821` | Read only, not run: the voice list is one `String` per speaker the model reports (`:576`, any `i32`), with no upper bound. Sample rate 0 and a negative count are refused or clamped (`:572-576`). | Nothing found: it needs a model whose metadata declares a huge speaker count, and I could not build one. | INFEASIBLE | introduced |
| 5 | `adapters/inference-sherpa-onnx-tts/src/lib.rs:702` | Read only: the pinned files are hashed by path (`:673-697`) and sherpa-onnx opens the same paths again. A file replaced in between is not seen. | Nothing found short of write access to the directory during `open`. Not fixable here: sherpa-onnx's configuration takes paths only. | INFEASIBLE | introduced |
| 6 | `adapters/inference-sherpa-onnx-tts/README.md:97` | The route the README gives a caller outside the workspace downloads the library with espeak-ng with no hash check (`sherpa-onnx-sys` `build.rs:199-209`). The README names neither the pin nor `SHERPA_ONNX_ARCHIVE_DIR`. | The README's own instruction. | CONFIRMED | introduced |
| 7 | `adapters/inference-sherpa-onnx/src/lib.rs:11` | "no GPL-licensed phonemiser is linked … never by the build itself" holds only inside this workspace. A consumer by Git dependency has no `SHERPA_ONNX_LIB_DIR`, so the build downloads `…-linux-x64-shared-lib` (`build.rs:129,392`), the build with TTS, for the STT adapter alone. `website/docs/status/limitations.md:48` says the same without the condition. Read, not built: downloads are forbidden here. | `website/docs/getting-started.md:58-66`. The text and the `sherpa-onnx-sys` pin are identical at the base (`git show`). | CONFIRMED | pre-existing |
| 8 | `website/docs/status/limitations.md:88` | "its record lists espeak-ng as a licence entry of its own": `records/tts-xeon-8375c-cpu.json` has no licence field. Option A of the decision blocker says the capability record and the README carry the entry. The entry is in the declared `Model`, the README and the adapters page. | A reader of the page who opens the record. | CONFIRMED | introduced |
| 9 | `adapters/inference-sherpa-onnx-tts/src/lib.rs:117` | "one entry per component": `CODE_LICENSES` names 4; upstream's list has 13 libraries (`build.rs:16-30`), among them `ucd`, `kissfft-float`, `kaldi-decoder-core`, `ssentencepiece_core`. The licences of the other nine were read neither by the unit nor by me. The test "espeak-ng is the one component under the GPL" (`tests/licence.rs:60`) checks the list against itself. | Anyone who relies on `code_license` being complete. | CONFIRMED | introduced |

Corrections, named and not applied:

- **Finding 1:** require `metadata.is_file()` for a file in `present`, and refuse a tree entry that is neither a file nor a directory.
- **Finding 2:** at least correct the doc of `open_voice`, the README section "A caller's own voice" and the limitations page. A typed refusal needs upstream to stop exiting, or a check outside the process.
- **Finding 3:** replace the predicate with the `names` rule of my file, or with `cargo metadata`.

## 4. The cases I added

`examples/local-voice-cascade/tests/security_w2u4_tts_boundary.rs`: 9 cases, one binary. `cargo fmt` and clippy with `-D warnings` are clean on it. None runs over 5 s.

| Case | Asserts | Now |
|---|---|---|
| `a_pinned_file_that_is_a_fifo_is_refused_and_is_not_opened` | `open` answers `FileMissing` for a FIFO `model.onnx` within 3 s | red (3.0 s) |
| `a_voice_whose_model_file_is_not_a_model_is_refused_and_the_process_lives` (ignored) | `open_voice` returns `BackendRefused` | red, SIGABRT |
| `a_voice_whose_model_is_of_another_family_is_refused_and_the_process_lives` (ignored) | the same, with the pinned Kokoro model as the voice's model | red, exit 255 |
| `a_directory_path_with_a_nul_byte_is_refused_by_both_constructors` | `FileMissing` with `InvalidInput`, no panic | green |
| `the_cascade_with_the_synthesiser_bound_maps_no_gpl_phonemiser_in_the_gate` | no phonemiser symbol in a mapped native library; `native_tts()` is `Absent` | green |
| `every_form_of_a_dependency_on_the_adapter_is_read_as_one` | the `names` rule reads 8 manifest forms and rejects 3 near-names | green |
| `only_the_offline_cascade_names_the_adapter_and_only_behind_its_feature` | no other manifest names the crate; optional; only `local`; not default; only examples depend on the cascade | green |
| `the_fetch_task_checks_each_archive_against_its_pin_before_it_unpacks_it` | URLs, both pins, and the order fetch, check, move, unpack | green |
| `no_other_task_and_no_workflow_runs_the_fetch_task` | no caller in `Taskfile.yml` or `.github/workflows/` | green |

First run of the cases alone, before any suite run (`cargo nextest run -p local-voice-cascade --all-features --test security_w2u4_tts_boundary`, exit 100):

```
    Starting 7 tests across 1 binary (2 tests skipped)
        PASS [   0.008s] (1/7) local-voice-cascade::security_w2u4_tts_boundary a_directory_path_with_a_nul_byte_is_refused_by_both_constructors
        PASS [   0.008s] (2/7) local-voice-cascade::security_w2u4_tts_boundary no_other_task_and_no_workflow_runs_the_fetch_task
        PASS [   0.009s] (3/7) local-voice-cascade::security_w2u4_tts_boundary every_form_of_a_dependency_on_the_adapter_is_read_as_one
        PASS [   0.010s] (4/7) local-voice-cascade::security_w2u4_tts_boundary the_fetch_task_checks_each_archive_against_its_pin_before_it_unpacks_it
        PASS [   0.011s] (5/7) local-voice-cascade::security_w2u4_tts_boundary only_the_offline_cascade_names_the_adapter_and_only_behind_its_feature
        PASS [   0.357s] (6/7) local-voice-cascade::security_w2u4_tts_boundary the_cascade_with_the_synthesiser_bound_maps_no_gpl_phonemiser_in_the_gate
        FAIL [   3.010s] (7/7) local-voice-cascade::security_w2u4_tts_boundary a_pinned_file_that_is_a_fifo_is_refused_and_is_not_opened
    thread 'a_pinned_file_that_is_a_fifo_is_refused_and_is_not_opened' (770568) panicked at examples/local-voice-cascade/tests/security_w2u4_tts_boundary.rs:111:19:
    open did not answer within 3 s for a directory whose model.onnx is a FIFO: a path that is neither a file nor a directory passes the check of its kind and is opened to be hashed
     Summary [   3.012s] 7 tests run: 6 passed, 1 failed, 2 skipped
```

The ignored cases, once, started on the library with TTS through the README's target runner (`<scratch>/run-ignored.sh`). The source path in sherpa-onnx's own message is shortened:

```
    Starting 2 tests across 1 binary (7 tests skipped)
        FAIL [   1.386s] (1/2) local-voice-cascade::security_w2u4_tts_boundary a_voice_whose_model_is_of_another_family_is_refused_and_the_process_lives
  stderr ───
    <sherpa-onnx>/csrc/offline-tts-vits-impl.h:InitFrontend:469 Not a model using characters as modeling unit. Please provide --vits-lexicon if you leave --vits-data-dir empty
    (test failed with exit code 255)
     SIGABRT [   4.110s] (2/2) local-voice-cascade::security_w2u4_tts_boundary a_voice_whose_model_file_is_not_a_model_is_refused_and_the_process_lives
  stderr ───
    fatal runtime error: Rust cannot catch foreign exceptions, aborting
    (test aborted with signal 6: SIGABRT)
     Summary [   4.111s] 2 tests run: 0 passed, 2 failed, 7 skipped
```

The predicate probe (`sh <scratch>/predicate-probe.sh`):

```
inline: read as a dependent
table: NOT read as a dependent
renamed: NOT read as a dependent
inherited: NOT read as a dependent
no-space: NOT read as a dependent
quoted: NOT read as a dependent
```

## 5. The suite, run after the cases existed

`<scratch>/run-suite.sh`, with the adversary's uncommitted binary deselected (`-E 'not binary(adversary_w2u4_synthesis)'`):

| Lane | Summary line | Exit |
|---|---|---|
| `cargo nextest run -p inference-sherpa-onnx-tts -p inference-sherpa-onnx --all-features` | `Summary [   0.746s] 81 tests run: 81 passed, 34 skipped` | 0 |
| `cargo nextest run -p local-voice-cascade --all-features`, my binary also deselected | `Summary [   1.284s] 28 tests run: 28 passed, 5 skipped` | 0 |
| the same, with my binary | `Summary [   3.034s] 35 tests run: 34 passed, 1 failed, 7 skipped` | 100 |
| `cargo nextest run -p inference-docs` | `Summary [   0.348s] 36 tests run: 35 passed, 1 failed, 0 skipped` | 100 |

- Before is 145 (81 + 28 + 36, the implementor's count, confirmed by the second lane); after is 152 (81 + 35 + 36).
- The docs red is the implementor's `every_binding_is_since_the_release_whose_changelog_section_introduced_it`, which waits on `CHANGELOG.md`.

## 6. For the coordinator

- **Finding 8:** the record schema is outside the unit's files; either it gains a licence field or `limitations.md:88` is reworded.
- **Finding 7:** pre-existing; yours to route.
- **The adversary's file:** `adversary_w2u4_synthesis.rs` fails `cargo clippy -p local-voice-cascade --all-targets --all-features -- -D warnings` (`while_let_on_iterator` at `:93`) and `cargo fmt --all --check` (two diffs). I saw this while linting my own file and did not touch theirs.

## 7. Reviewed and not faulted

- The fetch task's order and pins; the native pin equals the story's `3892d184…`, the checkpoint pin equals `ARCHIVE_SHA256`.
- `native_tts` on a deleted or unreadable library: `Unknown`, then sherpa-onnx returns nothing and the open is `BackendRefused`.
- The worker and gate life cycle (`src/synthesis.rs:139-194`, `:335-384`): every worker is told to stop before the last `Voice` drops; the join is bounded by one sentence per worker.
- `pcm16` clamping and NaN; frame length for any non-zero rate.
- The licence wording of `AGENTS.md:88-96`, the README table and the adapters page against the approval record's two points.

## 8. Paths written outside the worktree

All under `<scratch>/`:

- `metadata.json`, `metadata-all.json`, `metadata.err`
- `predicate-probe.sh`, `predicate-probe/` (6 `.toml` files)
- `run-ignored.sh`, `run-suite.sh`
- `run1-own-cases.log`, `run2-ignored-cases.log`
- `suite-summary.log`, `suite-sherpa.log`, `suite-cascade.log`, `suite-cascade-without-mine.log`, `suite-docs.log`, `suite-clippy-cascade.log`
- `clippy-own.log`, `fmt.log`

The cases write only under the worktree's own `target/tmp/security-w2u4-tts-boundary/` on the build node. Lease `w2-u4-security-1` is ended.

```findings
- file: adapters/inference-sherpa-onnx-tts/src/lib.rs
  line: 537
  category: integrity
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: open accepts a pinned path that is neither a file nor a directory and blocks hashing it, where its contract promises a FileMissing refusal
- file: adapters/inference-sherpa-onnx-tts/src/lib.rs
  line: 571
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: open_voice ends the process (SIGABRT or exit 255) on a voice whose model sherpa-onnx cannot use, where its documentation promises OpenRefused::BackendRefused
- file: adapters/inference-sherpa-onnx-tts/tests/licence.rs
  line: 214
  category: mutant
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the dependents check reads only the inline form of a dependency and misses the table, renamed, no-space and quoted forms, none of which a manifest uses today
- file: adapters/inference-sherpa-onnx-tts/src/lib.rs
  line: 821
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the speaker count a caller's model reports is unbounded and sizes the declared voice list, read and not run for want of a crafted model
- file: adapters/inference-sherpa-onnx-tts/src/lib.rs
  line: 702
  category: integrity
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the pinned files are hashed by path and opened again by sherpa-onnx by path, so a file replaced in between is not seen, and sherpa-onnx takes paths only
- file: adapters/inference-sherpa-onnx-tts/README.md
  line: 97
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the route given to a caller outside the workspace downloads the library with espeak-ng without a hash check and the README names neither the pin nor a checked alternative
- file: adapters/inference-sherpa-onnx/src/lib.rs
  line: 11
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: pre-existing
  message: the claim that no GPL-licensed phonemiser is linked and the build never fetches holds only inside this workspace, since a consumer by Git dependency gets the archive with TTS for the STT adapter alone
- file: website/docs/status/limitations.md
  line: 88
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the page says the adapter's record lists espeak-ng as a licence entry while the capability record file has no licence field
- file: adapters/inference-sherpa-onnx-tts/src/lib.rs
  line: 117
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: CODE_LICENSES is documented as one entry per component but names four of the thirteen libraries upstream lists, and the licences of the other nine were not read
```
