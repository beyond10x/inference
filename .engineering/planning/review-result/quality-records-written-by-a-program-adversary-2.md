---
format: aep.planning-md/3
id: review-result:quality-records-written-by-a-program-adversary-2
kind: review-result
status: active
title: Adversary pass 2, with the binary of the first correction, over the program that writes a quality record (2026-11-25)
relations:
- reviews: story:quality-records-written-by-a-program
revision: 1
---
unit: story:quality-records-written-by-a-program, `inference-docs assess`, worktree <worktree> at head 32949b8bb (correction 1f1efb601)
verdict: findings (3 critical, 4 warning, 3 note)
wrote-outside-worktree: 17 paths under <scratch>/evaluate/adversary-2/ (listed at the end)
cases: executed 95→100, red 5
origin: introduced 10 / pre-existing 0 / undecided 0
needs-coordinator: the red run went to a remote node (see below); `task test-run` for findings 16 and 13

**Remote node.** My second package run (the one with the added cases) printed `slot: job 67513 runs on node the build node`. I stopped building after it. It is the only run that shows the red cases. The run before it (`build-1.log`, the binary refresh you asked for) ran on this host.

**Worktree:** `git status --short` is empty. The cases were taken out after the run. The patch is `added-cases-pass2.patch`: the 4 cases of the earlier pass plus 1 of mine, and `git apply --check` passes.

**Order:** the package run you asked for came first. No case ran alone, because the brief allows only `cargo nextest run -p inference-docs`, so the red output comes from one package run with all 5 cases in it.

## Pass-1 findings, rerun with the current binary (`pass1.sh` → `pass1.out`)

- **1 fixed, with gaps (see 11, 12, 14).** `f1-gpt-failed` now exits 1 and names the failed test and the `test result: FAILED` line.
- **2 fixed, but the fix broke an honest case (see 13).** `f2-moved` now exits 1: "the report holds no test …status.rs::a_hosted_binding…".
- **3 fixed.** `file:///<an absolute path>/run.xml`, `../../../srv/x.xml`, `""` and `~/x.xml` now all exit 2, each with a reason. See 19.
- **4 fixed.** `f4-nested` now exits 1 with 4 lines (2 unplaced cases, tests 3≠1, failures 2≠0). A real cargo-nextest 0.9.143 report with a flaky test, a test that failed twice, a test killed at its timeout and an ignored test (`probe.junit.xml`) is read without refusal: executed 4, passed 2, failed 2, which agrees with the root.
- **5 fixed.** Both assessments now exit 2: "reads no single printed line of this measure".
- **6 partly fixed.** Comments, `panic!` text and a missing or misplaced marker are now caught. The check still never asks whether a value follows the label (see 15).
- **7 settled: true.** cargo-nextest 0.9.143 writes no ignored test into its JUnit report (see 16).
- **8 fixed.** `f8-record` now exits 1: "…as failed".
- **9 and 10:** both README sentences are true. One counting slip is note 20.

## New findings

**11. critical: a `test X ... ok` line of another test counts as the run's pass, so a run that was aborted or killed after the measures still gets a record.** `src/assess/printed.rs:166-170` (also 189-190), `src/assess.rs:1180-1194`.
- Input 1: `running 3 tests`, then `test one_spoken_exchange… ... ok`, then both measures, then the output ends. This is a kill.
- Input 2: the same, then `thread '<T>' (4123015) has overflowed its stack`, `fatal runtime error`, `error: test failed, to rerun pass …` and `(signal: 6, SIGABRT …)`.
- Output for both: exit 0, `passed: 1`, values 812 and 95 (`verdict/v2`, `verdict/v3`).
- What reaches it: the plan runs `cargo test … -- --ignored --nocapture`, which runs all ignored tests of one file in parallel. A pool kill, Ctrl-C or an abort ends that output without `test result:`.
- Accept: a pass needs a `test result: ok.` line with at least 1 passed (or a nextest `Summary` line with nothing failed). cargo's `error: test failed` line counts as a failure. Otherwise `--exit-status` is required.
- Case: `a_run_that_ended_before_the_measured_test_reported_gives_no_record`, red.

**12. warning: a cargo-nextest failure after a retry is never read.** `src/assess/printed.rs:178-194`.
- The status-line match needs the second word to start with `[`. A real 0.9.143 run with retries prints `TRY 2 FAIL [ … ]` and `TRY 2 TMT [ … ]` (`probe-run.log`, lines 51, 80, 191-192), so the match fails. `TMT` is also not in `NEXTEST_FAILED`.
- Input: try 1 fails before the measures, try 2 prints them and fails, another test passes, then `Summary … 1 failed`, `error: test run failed`.
- Output: exit 0, `passed: 1`. The `TMT` form (`more/m1`) gives the same.
- What reaches it: only a caller who runs a live test through nextest with `--retries`. The plan's commands use `cargo test`, which is why this is not critical.
- Accept: read `TRY <n> <status>` and `TMT`, and read the `Summary` counts.
- Case: `a_failure_that_cargo_nextest_reports_after_a_retry_gives_no_record`, red on its `TRY 2 FAIL` iteration. The `TMT` form is shown only with the binary.

**13. critical: an honest report is refused for `openai-time-bounds`.** `src/assess/junit.rs:390-397` (`Place::holds`).
- The named test `transport_deadlines.rs::a_server_that_never_answers_the_upgrade_fails_live_connect_within_the_deadline` sits in `#[cfg(feature = "gpt-live")] mod live {` (file line 334). nextest therefore names it `live::<fn>` in binary `transport_deadlines`. The exact match accepts only `<fn>` or `tests::<fn>`.
- Input: an all-passed report under nextest's names.
- Output: exit 1, "the report holds no test … of the package inference-openai".
- Before the correction this record came out byte-equal. Now 17 of 18 named-tests records do (`named-bytes.out`).
- Accept: the inline modules around the function, read from the file, are part of the expected path.
- Case: `a_named_test_in_an_inline_module_of_its_test_file_gives_the_committed_record`, red (left `Some(1)`, right `Some(0)`).

**14. warning: the panic reading never matches what the pinned rustc prints.** `src/assess/printed.rs:172-177`; the implementor's test at `tests/quality/assess.rs:995-1012`.
- rustc 1.100.0 (`rust-toolchain.toml`) prints `thread '<name>' (<tid>) panicked at`. `libstd` holds the pieces `thread '` and `) panicked at`, and every panic in the suite's own output has that form. The code splits on `' panicked`.
- The implementor's case uses the old form, so it passes; the README sentence at :63-64 ("a panic of a test the output names … is refused") is false for this toolchain.
- Input: the implementor's cut output in the real form. Output: exit 1, but only for "no `test result:` line", not for the panic.
- Accept: also match `' (<digits>) panicked`.
- Case: `a_panic_in_the_form_the_pinned_rustc_prints_is_read_as_a_failure_of_its_test`, red.

**15. warning: the label check passes a print the program cannot read.** `tests/quality/assess.rs:836-842` (`prints`).
- Only the label (and the marker, anywhere after it) must be in the literal; nothing checks that the value follows.
- Mutating deepgram's `"first event after {first:?}"` to `"first event after the start: {first:?}"` keeps `prints(...) == true`. The program then refuses that output: "holds no duration after `first event after`".
- The README sentence at :145-149 promises "with the text its value follows".
- Accept: in the literal, the label (or the marker) is followed by optional spaces and a `{…}` placeholder.
- Case: `the_label_check_fails_for_a_print_that_no_longer_puts_the_value_after_its_label`, red.

**16. critical: the program writes `skipped: 0` for `workspace-tests` from a real report of a run that skipped 72 tests.** `src/assess.rs:851-854`, `src/assess/junit.rs:5-6`, `quality/README.md:323-324`.
- cargo-nextest 0.9.143 leaves ignored tests out of the JUnit report. The probe log shows `SKIP … zz_probe_ignored` and `100 tests skipped`; its report has root `skipped="0"` and no `<skipped>` element.
- So the program's record says 0 where the committed record and the README say 72. Ratchet's format defines `skipped` as units left out on purpose.
- The fixture's `<skipped/>` case is a form nextest does not write.
- Accept: either `skipped` comes from a source that holds it, or nextest-run@1 drops `skipped` from its coverage, the README says the 72 comes from the log, and the fixture loses its `<skipped/>`.
- No case: it needs a decision first.

**17. warning: `cascade-talk` has no honest record.** `src/assess/printed.rs:86-97`, `quality/README.md:65-68`, test `assess.rs:1041-1054`.
- The example ends on Ctrl-C (status 130, refused). The only exit-0 path I found: `talk()` returns `Ok` when `drive()` returns, and `drive()` returns `Ok(())` when the run is Failed or Cancelled (`crates/inference-runtime/src/controller.rs:995`).
- Input `more/m2`: one turn, one interruption, `[egress to 203.0.113.7:443: the run failed]`, `[the run ended Failed]`, `--exit-status 0`. Output: exit 0, `passed: 1`.
- The README condition "only when it held one turn" is also incomplete: exactly one interruption is needed too.
- I traced the egress path by its printed text, not through `EgressWatch`.
- Accept: take `cascade-talk` out of `LABELS` (exit 2; the label count becomes 12), or read `[the run ended Completed]` as its verdict.

**18. note: colour codes turn a pass into a refusal.** `src/assess/printed.rs:153-158`.
- Input: `test result: \e[32mok\e[0m. 1 passed…`, as cargo prints with `--color always` or `CARGO_TERM_COLOR=always`.
- Output: exit 1, with that line quoted as a failure (`verdict/v4`).
- Accept: strip ANSI escape sequences before reading.

**19. note: `--source` edge cases.** `src/assess.rs:422-447`.
- Accepted: `https://user:secret@github.com/x`, which puts a credential in a public record; also `.`, `target/` and `https://github.com/x/../../y`.
- Refused: `run:1.xml` (a file name with a colon) and `HTTPS://…` (the scheme in capitals).
- Accepted as they should be: a query, a fragment, `./x`, `target/./x`, `.cache/x`, `a..b`.
- Accept: refuse URLs with user info; compare the scheme without regard to case.

**20. note: the README and the module doc miscount the program's own arguments.** `quality/README.md:41-47`, `src/assess.rs:4`. Both say "two of its own", and the README later says "these four". There are five: `--from`, `--source`, `--started-at`, `--ended-at`, `--exit-status`.

## What held

- Exit-status handling:
  - with a JUnit reader: exit 2;
  - `-1` and `256`: refused, since they are not 0;
  - a passed output with 101, and a failed output with 0: both refused;
  - `FAILED` inside a payload line: not counted;
  - `ok. 0 passed` from another binary: not counted as a pass;
  - an ignored-only run without measure lines: refused for the missing lines.
- Byte equality and determinism (`bytes.out`): `workspace-tests`, `adapter-tests`, `llama-server-live` and the committed `release-statuses` come out the same bytes twice and equal to their expected files. `sherpa-onnx-stt-record` is checked by the suite's own green test.
- All 57 named tests of the plans equal the tests of the 18 records. Only `transport_deadlines.rs` has an inline module other than `tests` (`named-places.out`).

## Suite

- Last run on this host, clean, at head: `Summary [  12.715s] 95 tests run: 95 passed, 0 skipped`.
- Red run (node the build node), with the 5 cases: `Summary [   0.959s] 100 tests run: 95 passed, 5 failed, 0 skipped`, exit 100.

## Not checked

- The nextest JUnit writer's source: it is not under `~/.cargo/registry/src`. Findings 12 and 16 rest on the earlier adversary's real 0.9.143 probe run on this host (`probe-run.log`, `probe.junit.xml`).
- The status word nextest prints for a timeout without retries (`TIMEOUT` or `TMT`), and when it writes `<error>`.
- Whether the CI report holds `live::…` for 13; the committed record says the test held.

Needs the workspace run (`task test-run`):
- `grep -c '<skipped'` against the log's 72 (finding 16);
- the default report's case count;
- the 120 s budget;
- the `stack` profile.

## Paths written

All under <scratch>/evaluate/adversary-2/:
- `added-cases-pass2.patch`
- logs: `build-1.log`, `red-run.log`
- reruns of the earlier scripts: `pass1.out` (+`p1/`), `source.out` (+`source/`), `verdict.out` (+`verdict/`), `named-places.out`, `named-bytes.out` (+`named/`), `bytes.out` (+`bytes/`)
- `record-tests.txt`, `evidence-format.md` (Ratchet's format, exported with `git show`)
- my probes: `probe-read.sh` with `probe-read/`, `more.sh` with `more/`

I deleted the stale `verdict/*.json` files with a glob (no variable in the path) before `verdict.sh` wrote them again.

```findings
- file: crates/inference-docs/src/assess/printed.rs
  line: 166
  category: acceptance
  severity: blocker
  verdict: CONFIRMED
  origin: introduced
  message: "a `test X ... ok` line of another test is read as the run's pass, so a cargo test run aborted or killed after the measured test printed its measures gets a record with exit 0 and passed 1"
- file: crates/inference-docs/src/assess/printed.rs
  line: 178
  category: boundary
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: "cargo-nextest 0.9.143 prints a failure after a retry as `TRY <n> FAIL` or `TRY <n> TMT`, which the status-line match never reads, so a failed nextest run with retries gets a record with exit 0"
- file: crates/inference-docs/src/assess/junit.rs
  line: 390
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "the exact match refuses a named test that sits in an inline `mod live` of its test file, so an all-passed real report of openai-time-bounds is refused where it was byte-equal before the correction"
- file: crates/inference-docs/src/assess/printed.rs
  line: 172
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: "the panic reading splits on `' panicked`, but rustc 1.100.0 prints `thread '<name>' (<tid>) panicked at`, so the branch never matches and the implementor's test pins a form the toolchain does not print"
- file: crates/inference-docs/tests/quality/assess.rs
  line: 836
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: "the label check passes a print that keeps the label but puts other words before the value, which the program then cannot read"
- file: crates/inference-docs/src/assess.rs
  line: 851
  category: contract-drift
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "cargo-nextest 0.9.143 leaves ignored tests out of its JUnit report, so the workspace-tests record the program writes says skipped 0 for a run that skipped 72"
- file: crates/inference-docs/src/assess/printed.rs
  line: 86
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: "cascade-talk ends on Ctrl-C, and its only exit-0 path includes a run that ended Failed, which the program writes as one passed run"
- file: crates/inference-docs/src/assess/printed.rs
  line: 153
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "a coloured `test result: ok` line is refused as a failure"
- file: crates/inference-docs/src/assess.rs
  line: 422
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "--source accepts a URL with user info, `.` and a directory, and refuses a file name with a colon and a scheme in capitals"
- file: quality/README.md
  line: 41
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "the README and the module doc say the program has two own arguments and later \"these four\", but it has five"
```
