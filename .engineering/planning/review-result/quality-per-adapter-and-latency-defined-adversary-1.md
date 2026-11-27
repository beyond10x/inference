---
format: aep.planning-md/3
id: review-result:quality-per-adapter-and-latency-defined-adversary-1
kind: review-result
status: active
title: Adversary pass on the per-adapter quality measures and the latency definitions
relations:
- reviews: story:quality-per-adapter-and-latency-defined
revision: 1
---
unit: story:quality-per-adapter-and-latency-defined, worktree at 864f27355fb7541881f9df279d61e33ee9536d2d plus my one new file and one `mod` line
verdict: red
cases: executed 65→72, red 7
origin: introduced 11, pre-existing 0, undecided 1
wrote-outside-worktree: 12 files under `<scratch>/` (listed in part 6)
needs-coordinator: no

## 1. `git --no-pager diff --stat`

```
 crates/inference-docs/tests/quality/mod.rs | 1 +
 1 file changed, 1 insertion(+)
```

`git status --short`:

```
 M crates/inference-docs/tests/quality/mod.rs
?? crates/inference-docs/tests/quality/adversary_q2.rs
```

The one changed line is `mod adversary_q2;`. The new file has 188 lines and passes `rustfmt --check`. No document, page or implementation was edited, and nothing was committed. Every defect was planted in memory through `Documents`; no file under attack was touched. The lease `quality2-adversary-1` is released.

## 2. The cases I added

File: `crates/inference-docs/tests/quality/adversary_q2.rs`. All seven are red now.

| Case | Asserts | Now |
|---|---|---|
| `a_number_in_a_paragraph_of_an_entry_without_a_baseline_is_a_problem` | the check reports "A run on a wired line gave 310 ms." as a paragraph under `cartesia-live/first-audio` | red |
| `a_number_in_a_bullet_without_a_name_of_an_entry_without_a_baseline_is_a_problem` | the same sentence as a bullet `- A run …` | red |
| `a_number_in_a_table_of_an_entry_without_a_baseline_is_a_problem` | a two-column table with `310 ms` under the entry | red |
| `a_test_named_only_in_a_comment_of_its_file_is_a_problem` | the Cartesia cancel test is renamed and a comment keeps `fn a_cancel_does_not_wait_for_the_server()` | red |
| `a_test_named_only_in_a_string_of_its_file_is_a_problem` | the same, with the old name in a string constant | red |
| `a_boundary_the_assessor_does_not_declare_is_a_problem` | `boundary: somewhere_to_somewhere` in the plan of `cartesia-live` | red |
| `the_coverage_of_the_adapter_tests_record_counts_the_tests_of_its_packages` | on the record as it is: `coverage.executed` equals the sum of its `tests_run` results | red |

Order of work: the file was written first. Its first version also held one temporary probe test, `table_of_planted_defects`, which I ran alone to fill the table in part 4b and then cut out. After that I ran the seven cases alone, then the suite.

Run of the seven cases alone, `cargo nextest run -p inference-docs -E 'test(/quality::adversary_q2::/)'`, exit 100. Each block also carried libtest's "running 1 test … test result: FAILED" stdout, left out here; full log `<scratch>/cases-alone.log`.

```
    Starting 7 tests across 2 binaries (65 tests skipped)
        FAIL [   0.005s] (1/7) inference-docs::workflows quality::adversary_q2::the_coverage_of_the_adapter_tests_record_counts_the_tests_of_its_packages
    thread '…' panicked at crates/inference-docs/tests/quality/adversary_q2.rs:182:5:
    assertion `left == right` failed: the coverage counts Some(3059) executed tests, and the `tests_run` results of the packages the plan names sum to 1166
      left: Some(3059)
     right: Some(1166)
        FAIL [   0.034s] (2/7) inference-docs::workflows quality::adversary_q2::a_number_in_a_table_of_an_entry_without_a_baseline_is_a_problem
    thread '…' panicked at crates/inference-docs/tests/quality/adversary_q2.rs:88:5:
    the check reports no problem for this planted defect: `cartesia-live/first-audio` says `Baseline: none.` and a table under it says 310 ms
        FAIL [   0.034s] (3/7) inference-docs::workflows quality::adversary_q2::a_test_named_only_in_a_comment_of_its_file_is_a_problem
    thread '…' panicked at crates/inference-docs/tests/quality/adversary_q2.rs:108:5:
    the check reports no problem for this planted defect: the test of `cartesia-time-bounds/held` was renamed and a comment keeps its old name
        FAIL [   0.034s] (4/7) inference-docs::workflows quality::adversary_q2::a_test_named_only_in_a_string_of_its_file_is_a_problem
    thread '…' panicked at crates/inference-docs/tests/quality/adversary_q2.rs:124:5:
    the check reports no problem for this planted defect: the test of `cartesia-time-bounds/held` was renamed and a string keeps its old name
        FAIL [   0.034s] (5/7) inference-docs::workflows quality::adversary_q2::a_number_in_a_paragraph_of_an_entry_without_a_baseline_is_a_problem
    thread '…' panicked at crates/inference-docs/tests/quality/adversary_q2.rs:58:5:
    the check reports no problem for this planted defect: `cartesia-live/first-audio` says `Baseline: none.` and a paragraph under it says 310 ms
        FAIL [   0.034s] (6/7) inference-docs::workflows quality::adversary_q2::a_number_in_a_bullet_without_a_name_of_an_entry_without_a_baseline_is_a_problem
    thread '…' panicked at crates/inference-docs/tests/quality/adversary_q2.rs:73:5:
    the check reports no problem for this planted defect: `cartesia-live/first-audio` says `Baseline: none.` and a bullet under it says 310 ms
        FAIL [   0.036s] (7/7) inference-docs::workflows quality::adversary_q2::a_boundary_the_assessor_does_not_declare_is_a_problem
    thread '…' panicked at crates/inference-docs/tests/quality/adversary_q2.rs:147:5:
    the check reports no problem for this planted defect: `cartesia-live/first-audio` names a boundary that `printed-duration@1` does not declare
     Summary [   0.036s] 7 tests run: 0 passed, 7 failed, 65 skipped
error: test run failed
exit 100
```

## 3. The suite run

`cargo nextest run -p inference-docs`, after the cases existed, exit 100. The seven failure blocks are the ones above; full log `<scratch>/suite.log`.

```
    Starting 72 tests across 2 binaries
        FAIL [   0.005s] (31/72) inference-docs::workflows quality::adversary_q2::the_coverage_of_the_adapter_tests_record_counts_the_tests_of_its_packages
        FAIL [   0.035s] (32/72) inference-docs::workflows quality::adversary_q2::a_boundary_the_assessor_does_not_declare_is_a_problem
        FAIL [   0.034s] (33/72) inference-docs::workflows quality::adversary_q2::a_number_in_a_bullet_without_a_name_of_an_entry_without_a_baseline_is_a_problem
        FAIL [   0.034s] (34/72) inference-docs::workflows quality::adversary_q2::a_number_in_a_paragraph_of_an_entry_without_a_baseline_is_a_problem
        FAIL [   0.037s] (37/72) inference-docs::workflows quality::adversary_q2::a_number_in_a_table_of_an_entry_without_a_baseline_is_a_problem
        FAIL [   0.035s] (38/72) inference-docs::workflows quality::adversary_q2::a_test_named_only_in_a_comment_of_its_file_is_a_problem
        FAIL [   0.034s] (39/72) inference-docs::workflows quality::adversary_q2::a_test_named_only_in_a_string_of_its_file_is_a_problem
     Summary [   0.544s] 72 tests run: 65 passed, 7 failed, 0 skipped
exit 100
```

The other 65 tests pass, among them the seven of `adapters.rs` and `the_quality_documents_agree_with_each_other_and_with_the_taskfile`.

## 4. Findings

All cover the tree at 864f27355.

| # | file:line | What is wrong | Source | Verdict | Origin | What reaches it |
|---|---|---|---|---|---|---|
| 1 | `quality/adapters.md:354` | The line "Close, text to speech: `CLOSE_BOUND`" names `a_cancel_does_not_wait_for_the_server` of `adapters/inference-elevenlabs/tests/cancel.rs`. The call it times, `discard`, reaches no code that reads `CLOSE_BOUND`. The test stays green with the bound removed or changed. It is one of the "7 of 7" and one result of the record. | `adapters/inference-elevenlabs/src/synthesiser.rs:808-827`: the doc comment says "The discard does no I/O and does not wait for the server". The crate reads `CLOSE_BOUND` only in `Drop` (`synthesiser.rs:773`), `connection.rs:150` and `agent_duplex.rs:658, 971`. The test that reaches `Drop`, `dropping_the_handle_mid_segment_closes_the_socket` (`cancel.rs:173`), asserts no time. The implementor's report moved the claim from that test to this one. | NEEDS-CHANGE | introduced | The reader of the page. A later Ratchet release reads `elevenlabs-time-bounds-held` as held for it (`quality/adapters.ratchet.yaml:209`). |
| 2 | `quality/records/adapter-tests.evidence.json:43` | The coverage is `executed 3059, passed 3059, skipped 72`, the counts of the whole workspace. The plan's `packages` input is the eleven adapter crates, whose `tests_run` results sum to 1166. The page says ignored tests have no count per crate. The 18 `named-tests@1` records of the same run count only their own tests. | `ratchet:spec/domains/evidence.yaml:220-229`: "How much of the plan a run executed". Failing case 7. | NEEDS-CHANGE | introduced | A later Ratchet release reading the documents. The check reads no coverage (changing `executed` to 1 stays green). |
| 3 | `crates/inference-docs/tests/quality/mod.rs:706` | `check_unobserved` reads only the bullets of the form `- <name>: <text>`. A latency number under a `Baseline: none.` entry passes as a paragraph, as a bullet without a name, or as a table. `quality/README.md:79-81` says "a line of it other than `Command:` and `Needs:` holds a digit". | Failing cases 1 to 3; `mod.rs:351-356` (a bullet enters `lines` only with `name: `). | NEEDS-CHANGE | introduced | The gate, on a later edit of the page: it stays green. At head no latency entry carries a number (I scanned all 25). |
| 4 | `crates/inference-docs/tests/quality/adapters.rs:334` | `check_test` is `code.contains("fn <name>(")`. A renamed test whose old name stays in a comment or a string passes. | Failing cases 4 and 5. | CONFIRMED | introduced | The gate, on a later rename that leaves the old name in the file. Nothing at head: all 41 named tests are real tests with a `PASS` line in the check run. |
| 5 | `crates/inference-docs/tests/quality/mod.rs:741` | `check_assessments` does not hold a measurement's `boundary` against the boundaries its assessor declares. The 25 latency measures share one output and differ in the YAML only by boundary. | Failing case 6; `quality/assessors/printed-duration@1.assessor.yaml:13-33`; `ratchet:spec/domains/assessor.yaml:104-108`. | CONFIRMED | undecided | The gate, on a misspelt boundary. Not run at the base. By reading, the base check has the same three mentions of `boundary` and no such rule, and the base plans already had boundaries. |
| 6 | `quality/adapters.md:572` | The clock of `llama-server-live-tools/generation` is said to end at "the end of the second turn". The test reads the clock when it prints, after `server_build` has made an HTTP request for `/props`. | `adapters/inference-llama-server/tests/live_tools.rs:183-193`: `println!("server build: {}", server_build(&base_url))` runs before `began.elapsed()` is evaluated. | NEEDS-CHANGE | introduced | The reader of the page. |
| 7 | `quality/adapters.md:539` | `llama-server-live-cancellation/slots-idle`: "the value is a multiple of it" is wrong. Each round is one HTTP request for `/slots` plus a 20 ms sleep. The first poll comes only after the test has drained and dropped the handle. | `adapters/inference-llama-server/tests/live_cancellation.rs:140-172`. | NEEDS-CHANGE | introduced | The reader of the page. |
| 8 | `quality/adapters.md:855` | "The same run writes the four keys of `compare` for the cascade." A cascade line has three: `first_transcript_ms` is written only for a native provider. It also has `tts_offer_to_audio_ms`, which no page names. | `examples/realtime-voice/src/compare.rs:2294-2303` (`Measure::applies`), `:2513-2520`, `:2286`. | NEEDS-CHANGE | introduced | The reader of the page. |
| 9 | `quality/adapters.md:403` | "Needs" leaves out variables the code reads. The ElevenLabs example reads `ELEVENLABS_RESIDENCY`; the probe entry at `:421` names it. The three OpenAI entries (`:659`, `:675`, `:690`) leave out `INFERENCE_OPENAI_SECRET_NAME`. Both are optional. | `adapters/inference-elevenlabs/examples/live.rs:87` (`Endpoint::from_env`); `adapters/inference-openai/tests/live_gpt.rs:62`, `tests/speech_live.rs:34`. | NEEDS-CHANGE | introduced | The reader of the page. |
| 10 | `quality/adapters.md:162` | The close lines that "compare the time with the constant" (Cartesia, Deepgram, Deepslate, ElevenLabs agent, Phonic) close against a local fixture that takes the write at once, so none fires the bound. `quality/not-measured.md:99` says a larger constant passes; it does not say a removed bound passes too. Cartesia waits up to `CLOSE_BOUND * 2` for the send while the test asserts `<= CLOSE_BOUND`. | `adapters/inference-cartesia/src/synthesiser.rs:463-486`; `tests/cancel.rs:112-129`. By reading only: I planted nothing in adapter code. | CONFIRMED | introduced | The reader of the page. |
| 11 | `quality/not-measured.md:291` | "A plan has no state that says 'defined, no observation yet'" (the implementor's Ratchet finding 2, which cites no line) leaves out that the specification names this state. What is missing is narrower: whether evidence was ever collected. | `ratchet:spec/domains/specification.yaml:850-851`: "An assessment with no requirement in any gate is in observe mode"; `ratchet:docs/design.md:597`. | NEEDS-CHANGE | introduced | A Ratchet maintainer reading the finding. |
| 12 | `quality/not-measured.md:285` | "A gate `release` would be the gate `check` under another name" understates. The gate the brief asked for requires what `check`, `slow` and the documentation gate require, and a free `stage` label lets one write that. Only the binding to the tag is missing. Leaving the gate out is what the brief ordered for that case. | `ratchet:spec/domains/specification.yaml:953-992` (Gate has no target field), `:227-229`, `:622-628`. | CONFIRMED | introduced | The reader of the page. |

The implementor's Ratchet findings 1, 3 and 4 are right at the pin with the lines they cite (`:953-992`, `:227-229`, `:622-628`, `:461-463`; for 4 `:692-727` and `:901-936`).

### 4b. Planted defects, one at a time

Run as one temporary test in one process: 33 defects, `PASS [0.961s]`. Output in `<scratch>/table.log`, source in `<scratch>/adversary_q2.with-table.rs`. "Red" means the check reports a problem.

| Planted defect | Check | Note |
|---|---|---|
| crate directory added under `adapters/` without a section | red | |
| section for a crate that does not exist | red | |
| named test renamed in its file | red | |
| named test moved to another file | red | |
| named test only in a comment | green | finding 4 |
| named test only in a string | green | finding 4 |
| named test nested in a module | green | no defect: the function is in the file |
| named test `#[ignore]`d | green | README says it is not held |
| named function no longer a `#[test]` | green | README says it is not held |
| constant's value changed on the page | green | README says it is not held |
| `--example` that does not exist, in the plan | red | |
| `--example` that does not exist, on the page | red | |
| required feature left out, in the plan | red | |
| required feature left out, on the page | red | |
| latency entry with a record added | red | |
| latency measure named by a requirement | red | |
| `Baseline: none.`, number in the `Bound:` line | red | |
| `Baseline: none.`, number in the `Clock:` line | red | |
| `Baseline: none.`, number in the `Needs:` line | green | README says these two lines are free |
| `Baseline: none.`, number in the `Command:` line | green | same |
| `Baseline: none.`, number in words ("under half a second") | green | not held, not listed |
| `Baseline: none.`, number in a paragraph under the entry | green | finding 3 |
| `Baseline: none.`, number in a bullet without a name | green | finding 3 |
| `Baseline: none.`, number in a table under the entry | green | finding 3 |
| latency baseline `310 ms` that names any file of the tree (`Cargo.toml`) | green | README says the number in a named file is not read |
| record value differs from the page's table | red | |
| per-crate value changed on the page only | red | |
| "holds for N of N tests" changed on the page only | red | |
| clock sentence changed | green | README says it is not held |
| billed line changed | green | README says it is not held |
| coverage of a record changed | green | not held, not listed; finding 2 |
| boundary the assessor does not declare | green | finding 5 |
| scope the assessor does not declare | red | reported only through the record no longer matching |

Not held and not on the README's list: findings 3, 4 and 5, the coverage of a record, and a number written in words. A feature gate written as `#![cfg(feature = …)]` in a test file is not read either; at head every named target has its features in the manifest's `required-features`.

### 4c. The step's time

- My suite run: 72 tests, `Summary [0.544s]`, 2.417 s summed. The seven tests of `adapters.rs` took 0.034 to 0.054 s each, 0.313 s summed. The slowest test was `every_generated_file_is_current_in_this_tree` at 0.285 s. The host's load average was 12 at that moment.
- No bound for this step alone stands in `quality/` or the gate. The only ones are `test-run-step/run-time` under 120 s for the whole task and 60 s per test.
- "Under 2 s" is in the implementor's brief only. Two of the four runs in its report are over it (2.769 s and 6.982 s); mine is under.
- A cheaper shape buys almost nothing. One process ran 33 defects on clones of one loaded set in 0.961 s, about 0.03 s per defect, which is what a whole one-defect test process costs here. The times the report saw came from the node's load.

## 5. Attacked and not broken

- Per-crate numbers: my own count from the check log matches the page and the record for all eleven crates (run, passed, slowest test and its time). The eleven sum to 1166, all 30 packages to 3059. The README's counting command prints the same eleven numbers.
- Time bounds: all 41 tests exist as `fn`, none is ignored, each has a `PASS` line in the check run. Every constant is in its file with the page's value, and every "at N ms" and "asserts the value" line is right, except finding 1.
- Record entries: each named test compares what the entry says; the llama-server test does not compare `adapter_revision`, as the page says.
- Latency commands (read, never run): every package, target file and feature exists and is required as written. Every quoted word or key is printed by the named source line. The clock sentences of Cartesia, Deepgram, both ElevenLabs entries, llama-server first token, both GPT-Live entries, OpenAI speech, `compare` and the cascade are right. "Billed" is right everywhere. No entry carries a number, a range or a "typically".
- Keys and enum values of the added YAML, the three assessor declarations and the 19 records fit the pinned specification. `run` as an implicit scope and a declared list input are allowed.
- `quality/README.md` on a release matches `AGENTS.md`, the tag trigger of `shared-gates.yml` and the gate `check`. `quality/not-measured.md`: nothing listed is produced by a command; the audio-device example prints a configured delay, not a latency.
- Public repository: no person's name, login, home path, host name, company, private repository or session name in the added lines or the commit message.

## 6. Paths written outside the worktree

All under `<scratch>/` (the directory `quality2/adversary-1` of the session scratch): `adversary_q2.final.rs`, `adversary_q2.with-table.rs`, `bodies.sh`, `bodies.txt`, `cases-alone.log`, `check.clean.log`, `main.lines`, `named.sh`, `named.tsv`, `perpkg.tsv`, `suite.log`, `table.log`. The builds ran in the shared pool's own directory through the `cargo` shim.

## 7. Findings block

```findings
- file: quality/adapters.md
  line: 354
  category: judgement
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the ElevenLabs text-to-speech close bound names a test whose timed call (discard) does no I/O and reaches no code that reads CLOSE_BOUND, so it stays green with the bound removed
- file: quality/records/adapter-tests.evidence.json
  line: 43
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the coverage counts the workspace (3059 executed, 72 skipped) while the plan names eleven packages whose tests_run results sum to 1166
- file: crates/inference-docs/tests/quality/mod.rs
  line: 706
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a latency number in a paragraph, a bullet without a name or a table under a Baseline none entry is not reported
- file: crates/inference-docs/tests/quality/adapters.rs
  line: 334
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a named test is found by substring, so a renamed test whose old name stays in a comment or a string passes
- file: crates/inference-docs/tests/quality/mod.rs
  line: 741
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: undecided
  message: a measurement's boundary is not held against the boundaries its assessor declares, and the 25 latency measures differ in the plans only by boundary
- file: quality/adapters.md
  line: 572
  category: judgement
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the clock of llama-server-live-tools/generation stops when the test prints, after an HTTP request for /props, not at the end of the second turn
- file: quality/adapters.md
  line: 539
  category: judgement
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the slots-idle value is not a multiple of the poll interval, since each round adds an HTTP request and the first poll follows the drain and drop of the handle
- file: quality/adapters.md
  line: 855
  category: judgement
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a cascade line of compare has three of the four keys, not four, because first_transcript_ms is written for native providers only
- file: quality/adapters.md
  line: 403
  category: judgement
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: Needs leaves out ELEVENLABS_RESIDENCY for the ElevenLabs example and INFERENCE_OPENAI_SECRET_NAME for the three OpenAI entries, both read by the code
- file: quality/adapters.md
  line: 162
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the close tests that compare a time with CLOSE_BOUND never fire the bound, so they pass with it removed as well as enlarged, and Cartesia waits up to twice the constant
- file: quality/not-measured.md
  line: 291
  category: judgement
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the claim that a plan has no state for defined and never observed leaves out observe mode, which the pinned specification names for an assessment with no requirement in any gate
- file: quality/not-measured.md
  line: 285
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a gate release would not be the gate check under another name, since the briefed gate also requires what slow and the documentation gate require and only the binding to the tag is inexpressible
```
