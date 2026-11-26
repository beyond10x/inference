---
format: aep.planning-md/3
id: review-result:quality-records-written-by-a-program-adversary-1
kind: review-result
status: active
title: Adversary pass 1, with the built binary, over the program that writes a quality record (2026-11-25)
relations:
- reviews: story:quality-records-written-by-a-program
revision: 1
---
unit: story:quality-records-written-by-a-program, `inference-docs assess`. Findings cover worktree `<worktree>` at f84a92540, plus b1ee7f5f7 (docs commit for AGENTS.md and run-the-checks.md, added while I worked)
verdict: findings (1 critical, 7 warning, 2 note)
wrote-outside-worktree: 26 paths under <scratch>/evaluate/adversary-1/ (listed at the end)
cases: executed 85→not run (4 cases written as a patch; nothing compiled under the build hold), red: not run
origin: introduced 10 / pre-existing 0 / undecided 0
needs-coordinator: a build to run added-cases.patch, and a `task test-run` to settle finding 7

**Worktree diff:** `git diff --stat` is empty and `git status` is clean. `git apply --check` of my patch passed; the patch was not applied.

**Binary used:** `target/debug/inference-docs`, 15:59:37. It is newer than every file under `crates/inference-docs/src/`.

**Permission refusal:** one command was stopped by the built-in safety check: "Dangerous rm operation detected in `rm -f $S/$id.json` … The target '$S/$id.json' is a shell variable expansion". I did not retry the removal. I wrote to new file names instead. Nothing was deleted.

## Findings

**1. critical: a live test that failed still gives a complete record with exit 0.** `src/assess.rs:1073-1080`, `src/assess/printed.rs:122-128`.
- The printed reader looks only for `skipped:` lines and never reads the test's verdict. It always writes `executed 1, passed 0, failed 0`, but the declaration's `coverage` lists `passed` and `failed`.
- Input: `gpt-live-failed.out`. It has `started in 812 ms…`, `closed in 95 ms: Err(Timeout)`, then a panic, `test … FAILED` and `test result: FAILED. 2 passed; 1 failed`.
- Call: `assess --assessment openai-gpt-live --class local --from gpt-live-failed.out --started-at … --ended-at …`
- Output: exit 0, `completion: completed`, coverage failed 0, values `812` and `95`.
- Reachable in a real run: `adapters/inference-openai/tests/live_gpt.rs:257-272` and `adapters/inference-deepgram/tests/live.rs:109-115` both assert after printing their measures.
- Accept: the program reads `test result:` and the per-test `... FAILED` lines. A failure is either refused (exit 1) or written as `failed: 1`, and a pass as `passed: 1`.

**2. warning: a test of the same name in another module counts as the named test (`held: true`).** `src/assess/junit.rs:289-308`.
- `holds` only checks that the module path starts right and the name ends right, so any modules in between are accepted. For `tests/<bin>.rs` the module path is not checked at all.
- Input: the fixture report with `status::tests::a_hosted_binding_…` renamed to `status::legacy::tests::a_hosted_binding_…`.
- Call: `assess --assessment release-statuses --class ci --revision 59b8f4a8… --from moved.junit.xml --source <run URL>`
- Output: exit 0, `holds: true`. The file is byte-equal to the committed `release-statuses` record, although no test of `src/status.rs` with that name ran.
- What else guards it: the quality test fails when a named function leaves its file. That covers a move, but not a named test that is still in its file and did not run (cfg-gated) while a same-named test elsewhere did.
- Accept: the case name equals `<modules>::<function>` or `<modules>::tests::<function>`, and nothing else matches.

**3. warning: `--source` takes an absolute path inside a URL, a path that leaves the root, and an empty string.** `src/assess.rs:392-409`.
- Ratchet's evidence format, § 4 rule 9, forbids an absolute path in `evidence`.
- Calls on `workspace-tests` with `--source file:///<an absolute path>/run.xml`, `../../../srv/x.xml`, `""` and `~/x.xml`.
- Output: all four exit 0, each with `source: {kind: junit, location: <as given>}`.
- Accept: exit 2 unless the value is an http(s) URL or a non-empty path relative to the root with no `..`.

**4. warning: test cases the reader does not place are dropped without notice, and the report's own totals are never compared.** `src/assess/junit.rs:110-118`, `junit.rs:170-175`, and the doc comment at `junit.rs:8-10`.
- Input: `nested.junit.xml`. The root says `tests="3" failures="2"`. It has one failed case directly under `<testsuites>`, one failed case in a nested suite, and one passed case.
- Output: exit 0, `executed 2, failed 1`, slowest `9.500`.
- The doc comment is also wrong: cases in a nested suite are read.
- Accept: refuse (exit 1) when the counted cases disagree with the root's `tests`/`failures`/`errors`/`skipped`. The same check would also surface finding 7.

**5. warning: two assessments the program claims to read are refused on every complete honest run.** `src/assess/printed.rs:34-39`, `printed.rs:96-107`, `printed.rs:133-143`.
- `elevenlabs-live-example`: the example prints one `first audio after` line per format, two in all (`examples/live.rs:56-58`).
- Output on `elevenlabs-example.out`: exit 1, "2 lines of the output hold `first audio after`".
- `cascade-local-adapters`: three tests call `print_measures` (`tests/local_adapters.rs:822`, `:946`, `:1096`), and one of them runs two turns. A `--ignored` run therefore always prints the label several times.
- The test file's own case at `tests/quality/assess.rs:429-437` asserts this refusal as correct, while `assess.rs:805` counts these measures among the 17 that are read.
- Accept: either exit 2 ("reads no printed line"), as for `compare`, or a stated rule for which line counts, with the 17 count and the README matching it.

**6. warning: the label-in-source test cannot fail for 3 of the 17 labels, and it never checks the `after` markers.** `tests/quality/assess.rs:796-799`.
- It uses `source.contains(label)`, which a comment or panic message satisfies.
- Probe: copies of the sources with the printing lines removed, in scratch. `talk-mutated.rs` still holds both cascade labels once each, from the module comment at lines 25-26. `live_cancellation-mutated.rs` still holds `first delta` 3 times (lines 13, 24 and the panic at 239).
- The `after` markers (`" after "`, `"first audio"`, `":"`) are never checked against the source.
- `quality/README.md` says the test catches "a text the source of its command does not print", which is a stronger claim than the test makes.
- Accept: match the label inside a `println!`/`format!` string literal, and include the marker.

**7. warning, undecided until a run: whether a real nextest report holds skipped tests at all.** `src/assess/junit.rs:124-130`, `src/assess.rs:669-681`.
- The implementor marked this unverified, and the only real report here has 0 skipped.
- If nextest 0.9.143 leaves ignored tests out of the JUnit report (my recollection of nextest-runner, not checked), a program-written `workspace-tests` record says `skipped: 0` where `quality/README.md:296-297` states 72.
- In that case `package-tests` also refuses any package whose tests are all ignored ("no test … that ran") instead of writing `tests_run 0`.
- What a run must show: a workspace `task test-run` report with `grep -c '<skipped' target/nextest/default/junit.xml` compared with the 72 in the log.

**8. warning (judgement): when the capability record's test fails, the record still gets a complete value.** `src/assess.rs:979-1012`.
- Input: `record.junit.xml`, where the sherpa `stt_record` test has `<failure>`.
- Output: exit 0, value `131` (kind exact), coverage `failed: 1`. A skipped test of the same record is refused with exit 1.
- The plan sets no coverage bound, so `sherpa-onnx-stt-record-keeps-up` passes on a record whose own check failed.
- Accept: a failed record test is refused like a skipped one, or the README says the value stands.

**9. note: Ratchet's own call cannot run any assessment of this program.** `src/assess.rs:1043-1047`, `quality/README.md:41-44`.
- `--from` and `--source` are required, and printed-duration also needs `--started-at`/`--ended-at`. § 10 says "Ratchet passes no argument beside these", so every call through Ratchet exits 2.
- The implementor already reported this. It is listed here so it reaches the review.

**10. note: the `profile` input is never compared with the report read.** `src/assess.rs:751`.
- `workspace-tests` given a report of the `slow` or `stack` profile writes `inputs.profile: default`.

## Cases written (not run; they need a build)

All in `added-cases.patch`, against `crates/inference-docs/tests/quality/assess.rs` (`git apply --check`: exit 0):
- `a_test_of_the_same_name_in_another_module_does_not_hold_a_named_test`: expects exit 1. The binary on the same input gives exit 0 with `holds: true` (finding 2).
- `the_output_of_a_live_test_that_failed_gives_no_record_without_the_failure`: expects exit 1 or `failed: 1`. The binary gives exit 0 with `failed: 0` (finding 1).
- `a_source_that_is_neither_a_web_url_nor_a_path_in_the_root_is_a_wrong_call`: expects exit 2. The binary gives exit 0 (finding 3).
- `a_report_whose_root_counts_disagree_with_the_cases_read_is_refused`: expects exit 1. The binary gives exit 0 with `failed: 1` (finding 4).

## What held

- **Byte equality:** my script `named-bytes.sh` built an all-passed report for each of the 18 committed named-tests records and ran the program with each record's class, revision and source. All 18 came out byte-equal. `whispercpp-stt-record` from the real tree file is byte-equal too. `workspace-tests`, `slow-lane`, `adapter-tests` and `sherpa-onnx-stt-record` have the same shape as the program's output; only values from the log differ.
- **Expected fixtures:** I checked `workspace-tests` and `adapter-tests` by hand against `report.junit.xml`, including counts, slowest, failed-but-slowest, flaky counted as passed, skipped excluded, and the run times. All correct.
- **Determinism and safety by reading:** no clock, environment variable, host, process or socket in `assess*.rs`. Output order follows the plan. A capability record's digest and its value come from one read.
- **Exit codes** match Ratchet § 10: a wrong `--class`, an unknown assessment or an assessor without a reader gives 2; an unreadable file gives 1.
- **Dependencies:** `sha2` 0.11.1 and `xmlparser` 0.13.6 were already in `Cargo.lock`; only two dependency lines were added. Both are MIT/Apache-2.0 by my knowledge; not checked against a deny list, as the repository has none.
- **Profile change, by reading:** `[profile.stack]` keeps the 60 s kill, and the default `junit.xml` is no longer overwritten. `test-slowest` still reads the default file.
- **Repository rules:** no name, private path, duration or Python in the diff or the commit messages. No off-limits file changed.

## Not checked, or needs a build

- The 4 cases above have not compiled or run. The suite count after them is unknown.
- `task test-run` with the `stack` profile: the default report should keep about 3000 `<testcase>`, `target/nextest/stack/` should exist if custom profiles inherit `junit` from `default`, and the step should end within 120 s.
- Finding 7 (what nextest writes for ignored or filtered tests), and what it writes for a test killed at its timeout.
- I did not read other worktrees' reports; the brief puts them off limits.

## Paths written

All under <scratch>/evaluate/adversary-1/:
- the patch and script: `added-cases.patch`, `named-bytes.sh`
- `named/`: 54 files (18 reports, 18 documents, 18 stderr logs)
- Ratchet exports via `git show`: `evidence-format.md`, `cli.yaml`, `design.md`
- probe inputs: `real.junit.xml`, `moved.junit.xml`, `nested.junit.xml`, `record.junit.xml`, `skipped-nextest.junit.xml`, `gpt-live-failed.out`, `elevenlabs-example.out`
- mutated source copies: `talk-mutated.rs`, `live_cancellation-mutated.rs`
- documents the binary wrote: `ws.json`, `moved.evidence.json`, `nested.evidence.json`, `gpt-live-failed.evidence.json`, `cap-sherpa-onnx-stt-record.json`, `cap-whispercpp-stt-record.json`, `src-file____home_someone_run_xml.json`, `src-.json`, `src-_________srv_x_xml.json`, `src-__x_xml.json`

```findings
- severity: blocker
  file: crates/inference-docs/src/assess.rs
  line: 1073
  category: acceptance
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "printed-duration writes a complete record with exit 0 and coverage failed 0 from the output of a live test that FAILED after printing its measures"
- severity: warning
  file: crates/inference-docs/src/assess/junit.rs
  line: 289
  category: boundary
  verdict: CONFIRMED
  origin: introduced
  message: "named-tests matches by module prefix plus name suffix, so a same-named test in another module or file is written as held for a named test that did not run"
- severity: warning
  file: crates/inference-docs/src/assess.rs
  line: 392
  category: contract-drift
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "--source accepts file:/// URLs with absolute paths, paths with .. and an empty string, against rule 9 of the evidence format"
- severity: warning
  file: crates/inference-docs/src/assess/junit.rs
  line: 110
  category: boundary
  verdict: CONFIRMED
  origin: introduced
  message: "a test case outside a testsuite is dropped without notice and root totals are never cross-checked, so the counts are wrong with exit 0; the doc comment on nested suites is also wrong"
- severity: warning
  file: crates/inference-docs/src/assess/printed.rs
  line: 34
  category: acceptance
  verdict: CONFIRMED
  origin: introduced
  message: "elevenlabs-live-example and cascade-local-adapters are counted as read but every complete honest run prints the label several times and is refused"
- severity: warning
  file: crates/inference-docs/tests/quality/assess.rs
  line: 796
  category: mutant
  verdict: CONFIRMED
  origin: introduced
  message: "the label-in-source test passes on doc comments or panic text for 3 of 17 labels and never checks the after-markers, weaker than the README sentence"
- severity: warning
  file: crates/inference-docs/src/assess/junit.rs
  line: 124
  category: contract-drift
  verdict: CONFIRMED
  origin: introduced
  message: "unverified whether nextest writes ignored tests as skipped; if not, workspace-tests gets skipped 0 instead of the 72 the README states, and package-tests refuses an all-ignored package"
- severity: warning
  file: crates/inference-docs/src/assess.rs
  line: 979
  category: judgement
  verdict: CONFIRMED
  origin: introduced
  message: "a failed capability-record test still gives a complete value with exit 0, while a skipped one is refused"
- severity: note
  file: crates/inference-docs/src/assess.rs
  line: 1043
  category: contract-drift
  verdict: INFEASIBLE
  origin: introduced
  message: "required --from and --source, and the times for printed-duration, are beyond Ratchet's argument list, so every call through Ratchet exits 2"
- severity: note
  file: crates/inference-docs/src/assess.rs
  line: 751
  category: judgement
  verdict: CONFIRMED
  origin: introduced
  message: "the profile input is written from the plan and never compared with the report that was read"
```
