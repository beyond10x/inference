---
format: aep.planning-md/3
id: review-result:release-check-for-added-ignores-adversary-1
kind: review-result
status: active
title: Adversary pass over the check of ignore reasons (2026-11-26)
relations:
- reviews: story:release-check-for-added-ignores
revision: 1
---
unit: U4 story:release-check-for-added-ignores, worktree inference-wave-u4, head 5d6e1adea (branch wave/u4-ignore-check)
verdict: NEEDS-CHANGE. 3 warnings, 5 notes, 0 blockers
cases: executed 127→137, red 9
origin: introduced 8 / pre-existing 0 / undecided 0 (the check file does not exist at base 7f06f0b59)
wrote-outside-worktree: 5 paths (part 6)
needs-coordinator: none

## 1. Diff stat (while my cases were in; the tree is restored now and `git status --short` is empty)
```
 crates/inference-docs/tests/ignores/adversary.rs | 136 +++++++++++++++++++++++
 crates/inference-docs/tests/ignores/mod.rs       |   2 +
```
Both paths are test files. The only change to mod.rs is one appended line, `mod adversary;`. No implementation line changed. The cases are saved as `added-cases.patch` in my scratch directory, and `git apply --check` passes on 5d6e1adea.

## 2. Cases (file `crates/inference-docs/tests/ignores/adversary.rs`, run alone with `cargo nextest run -p inference-docs -E 'test(/ignores::adversary::/)'`, result `10 tests run: 1 passed, 9 failed`, exit 100)
| Case | Planted input | Red output |
|---|---|---|
| a_long_cfg_attr_ignore_as_rustfmt_writes_it_is_read_and_refused | `#[cfg_attr(\n    all(...),\n    ignore = "flaky on a busy runner"\n)]` | `read 0, refusals [] left: (0, 0) right: (1, 1)` (adversary.rs:26) |
| an_ignore_in_a_test_module_directory_named_target_is_read_and_refused | `tests/target/mod.rs` holding `#[ignore]` | `read 0, refusals [] left: (0, 0) right: (1, 1)` (:35) |
| a_hosted_live_test_that_names_its_clip_variable_and_drops_billed_is_refused | the real elevenlabs `Cargo.toml` and `stt_live.rs`, with the reason changed to "calls ElevenLabs speech-to-text on the clip in INFERENCE_ELEVENLABS_STT_CLIP" | `read 1, refusals [] left: 0 right: 1` (:57) |
| a_child_whose_name_stands_only_in_a_comment_is_refused | the name appears only in `// planted_case is run by nobody` | `[] left: 0 right: 1` (:66) |
| a_child_whose_name_is_only_a_prefix_of_another_function_is_refused | `fn case` beside `fn case_two`. The word "cases" in the reason also counts. | `[] left: 0 right: 1` (:75) |
| a_local_variable_the_package_names_only_in_a_comment_is_refused | `// INFERENCE_PLANTED_MODEL is not read yet` | `[] left: 0 right: 1` (:84) |
| a_local_variable_that_is_only_a_prefix_of_one_the_package_reads_is_refused | reason `INFERENCE_PLANTED`, package reads `INFERENCE_PLANTED_DIR` | `[] left: 0 right: 1` (:93) |
| limit_and_by_hand_do_not_admit_a_flaky_reason | "limit: flaky on a busy runner" | `limit: flaky on a busy runner: [] left: 0 right: 1` (:103) |
| not_billed_does_not_admit_a_hosted_reason | "calls the hosted API, not billed on the free tier; needs a key" | `[] left: 0 right: 1` (:113) |
| forms_the_check_refuses_or_reads_as_it_should (green) | raw string reason, reason on the next line, "credentials", "money", "keys", an ignore in `src/` and one in `examples/` | passes: each of these is refused or read as it should be |

## 3. Suite run (after the cases existed)
- `cd <worktree> && cargo nextest run -p inference-docs --no-fail-fast`: `Summary [ 1.290s] 137 tests run: 128 passed, 9 failed, 0 skipped`, exit 100. The 9 failures are exactly the red cases above.
- Without my cases (`-E 'not test(/ignores::adversary::/)'`): `127 tests run: 127 passed, 10 skipped`, exit 0. This is the source of the before count, 127.
- Cost. The tree test alone (`-E 'test(=ignores::every_ignore_of_the_workspace_gives_a_reason_of_an_admitted_kind)'`) took `PASS [ 0.380s]`, exit 0. That is under the 1 s bound.

## 4. Findings
F1. **A long `cfg_attr` ignore is not read** (warning, NEEDS-CHANGE, introduced). `is_ignore` looks at one line at a time (mod.rs:89-93, :100).
- rustfmt (edition 2024, max width 100) splits a `#[cfg_attr(<predicate>, ignore = "<reason>")]` over several lines once it is longer than 100 characters. A hosted reason of normal length already crosses that.
- The `ignore = ...` line then starts without `#[`, so the attribute is not read and any reason, or none, passes.
- The claim "including a `#[cfg_attr(…, ignore…)]`" and AGENTS.md:97 ("reads each `#[ignore]`") do not hold for this shape.
- What reaches it: rustfmt itself, and `cargo fmt --all --check` in check-source forces that shape. No such attribute is in the tree today.
- Fix: when a trimmed line starts `#[cfg_attr(`, join lines until the brackets balance, then look for an `ignore` item and take the reason from the joined text.

F2. **Every directory named `target` is skipped, at any depth** (warning, NEEDS-CHANGE, introduced). The cause is the name test at mod.rs:79.
- What reaches it: `crates/inference-testkit/tests/target/` holds 10 test source files. It is `mod target;` in at least 12 test binaries (adversary1_target.rs, target_self.rs and others).
- An `#[ignore]` added there is never read. It holds none today.
- Fix: skip only `<root>/target`, or a directory that holds `CACHEDIR.TAG`.

F3. **A hosted live test passes as local** (warning, NEEDS-CHANGE, introduced). The cause is the kind order at mod.rs:184-223.
- The case uses the real elevenlabs stt_live test. With `billed` and the key word dropped from its reason, the check admits it because the reason names `INFERENCE_ELEVENLABS_STT_CLIP`.
- What reaches it: four hosted packages read `INFERENCE_*` variables, so a reason that names only that variable bypasses the billed rule:
  - deepgram reads `INFERENCE_DEEPGRAM_STT_CLIP`;
  - aws reads `INFERENCE_NOVA_SONIC_MODEL`;
  - openai reads `INFERENCE_OPENAI_REALTIME_MODEL`;
  - elevenlabs reads `INFERENCE_ELEVENLABS_STT_CLIP`.
- The implementor named this gap in their own report.
- Fix: refuse any ignore in a test target whose `[[test]]` has `required-features` with `live`, or whose file has `#![cfg(feature = "live")]`, unless its reason says `billed`. All 22 billed tests today are gated this way (checked), so the tree stays green.

F4. **The child check matches a substring of the file** (note, introduced). It is `file.matches(name).count() > 1` at mod.rs:201.
- A comment, a longer identifier, or the reason text ("cases" contains `case`) counts as the parent running the case.
- Fix: count whole-identifier hits outside comments and outside the function's own declaration, for example only a string literal `"name"`.

F5. **"Its package reads" means "mentions anywhere"** (note, introduced). The cause is `package.contains(variable)` at mod.rs:217.
- A comment or a longer variable counts as a read. Example: elevenlabs src/lib.rs:201 names its variable only in a `//!` comment; the real read is the const in the test file.
- AGENTS.md:103 says "reads".
- Fix: match the whole word inside a string literal, with comments removed.

F6. **`limit:` and `by hand` admit any text** (note, judgement, introduced). The check at mod.rs:196 is a prefix test and a substring test.
- "limit: flaky on a busy runner" passes. AGENTS.md:110 asks for "a limit the code's documentation states", and nothing checks that.
- Fix (judgement): a `limit:` reason names a story id or a doc anchor that exists.

F7. **"not billed" counts as billed** (note, introduced). The cause is the word test at mod.rs:184.
- It also skips the hosted refusal, as the case shows.
- Fix: refuse `billed` when `not` comes right before it.

F8. **A mutant the existing suite does not catch** (note, mutant, introduced). The hosted words at mod.rs:188 are partly untested.
- Dropping `credentials`, `money` or `keys` from the list survives the implementor's cases. The only tree reasons with those words now also carry `billed`.
- I did not run a mutated copy; this comes from reading the cases.
- My green case pins all three words.

AGENTS.md compared with the code: the five kinds and the refusal text match. The drift is "reads each `#[ignore]`" (F1, F2) and "its package reads" (F5).

## 5. Attacked and could not break
- Forms that rustfmt normalises and `cargo fmt --all --check` refuses are unreachable in a commit, so the gate covers them: `#[test] #[ignore]` on one line, `# [ignore]`, `cfg_attr(x,ignore)` without a space, a short `cfg_attr` spread over lines, `#[ignore="x"]`, and a reason on the next line. Checked with rustfmt on a scratch file.
- A raw string reason and a reason on the next line are refused (the safe direction).
- Ignores under `src/` (unit tests) and `examples/` are read. The build `target/` is skipped. There is no vendored directory and no symlink in the tree.
- `#![ignore]` does not ignore a test, so it is out of scope.
- `unbilled` is not counted as billed (whole-word match).
- An escaped quote inside a reason is handled.
- Every billed test is behind feature `live`: required-features `live` on its target, or `#![cfg(feature = "live")]` in aws. The gpt-live tests that are not ignored are offline (local WebSocket server).
- The tree test takes 0.38 s.

## 6. Paths written outside the worktree
All in `<scratch>/wave-1126/u4-adversary/`:
- `added-cases.patch`
- `cases-alone.log`
- `suite.log`
- `suite-without.log`
- `tree-alone.log`

`fmt-probe.rs` was written there too and has been deleted. The trees the cases planted were made under the worktree's `target/tmp` and removed by the cases.

```findings
- file: crates/inference-docs/tests/ignores/mod.rs
  line: 89
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "A cfg_attr ignore longer than 100 columns, which rustfmt splits over lines, is not read at all, so any reason or none passes."
- file: crates/inference-docs/tests/ignores/mod.rs
  line: 79
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "Every directory named target is skipped at any depth, so an ignore added under crates/inference-testkit/tests/target/, a module of at least 12 test binaries, is never checked."
- file: crates/inference-docs/tests/ignores/mod.rs
  line: 209
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "A hosted live test whose reason names an INFERENCE_* variable its package reads, without billed or a key word, is admitted as local; the feature-live gate of the test target is not used."
- file: crates/inference-docs/tests/ignores/mod.rs
  line: 201
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The child kind counts any substring of the file, so a comment, a longer identifier or the reason text stands in for a parent that runs the case."
- file: crates/inference-docs/tests/ignores/mod.rs
  line: 217
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "The package counts as reading a variable when any non-ignore line mentions it as a substring, including comments and longer names, while AGENTS.md says the package reads it."
- file: crates/inference-docs/tests/ignores/mod.rs
  line: 196
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "A limit: prefix or the words by hand admit any reason, such as limit: flaky on a busy runner."
- file: crates/inference-docs/tests/ignores/mod.rs
  line: 184
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "A reason saying not billed is admitted as billed and skips the hosted refusal."
- file: crates/inference-docs/tests/ignores/mod.rs
  line: 188
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "No implementor case holds credentials, money or keys, so dropping any of them from the hosted word list survives the suite."
```
