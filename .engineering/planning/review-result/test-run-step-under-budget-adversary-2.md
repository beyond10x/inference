---
format: aep.planning-md/3
id: review-result:test-run-step-under-budget-adversary-2
kind: review-result
status: active
title: Adversary pass 2 on the test budget unit
relations:
- reviews: story:test-run-step-under-budget
revision: 1
---
Adversary, pass 2 (the last), on story:test-run-step-under-budget, 2026-11-24, as the adversary returned it after the store refused the severity `low` in its first findings block; `<scratch>` is the pass's scratch directory outside the repository.

unit: story:test-run-step-under-budget
verdict: red
cases: executed 1029→1033, red 1
origin: introduced 4, pre-existing 0, undecided 0
wrote-outside-worktree: <scratch>/ (files listed in section 6); build node job `target/` (sherpa-onnx archive, `target/tmp/conformance-suite`)
needs-coordinator: yes

What "red" stands on: one test got faster by checking less (N1, low severity, a mutant survives all 111 existing realtime-voice tests), and pass 1's F3 is carried. The one red case at head (N2) is INFEASIBLE: nothing in the tree reaches it. Everything else attacked held.

## 1. `git --no-pager diff --stat`

Empty: no tracked file differs from head `7ce339d92d1c58c23fa3032713df10c098cac6ca`. `git status --short`:

```
?? adapters/inference-openai/tests/adversary2_w1_script_end.rs
?? examples/realtime-voice/tests/adversary2_w1_key_check.rs
```

Stated plainly: under the brief's allowance I applied three batches of temporary `src` mutants (M1, M2, M3; 13 edits in `adapters/inference-openai/src/{live,live_duplex,duplex}.rs`, `adapters/inference-deepgram/src/agent_duplex.rs`, `examples/repo-podcast/src/show.rs`, `examples/realtime-voice/src/elevenlabs.rs`). Each batch was recorded (`<scratch>/m1.diff`, `m2.diff`, `m3.diff`) and reversed with `git apply -R`; the status above is after the last reversal. Three temporary copies of existing testkit tests (`crates/inference-testkit/tests/adversary2_w1_tmp_whole_*.rs`, Round 2) were created, run and removed. No existing test was edited. No commit, stash, branch or push. Lease `w1-adversary-2` released (`session-end` exit 0); worktree left in place.

## 2. Cases added (uncommitted)

`adapters/inference-openai/tests/adversary2_w1_script_end.rs` (3 tests; script: `response.created resp_1`, `response.done`, the `error` marker, a pause, `response.created resp_late`):
- `adversary2_w1_the_read_to_the_marker_sees_a_response_created_at_once_after_it`: PASS 0.059 s
- `adversary2_w1_the_two_second_read_sees_a_response_created_300_ms_after_the_marker` (control, the read before the correction): PASS 2.342 s
- `adversary2_w1_the_read_to_the_marker_sees_a_response_created_300_ms_after_it`: FAIL 0.159 s

```
thread 'adversary2_w1_the_read_to_the_marker_sees_a_response_created_300_ms_after_it' (3458085) panicked at adapters/inference-openai/tests/adversary2_w1_script_end.rs:259:5:
assertion `left == right` failed: the response the server created 300 ms after the marker was not read: [
    ResponseCreated(
        ResponseCreated {
            response_id: ResponseId(
                3db714d9-3203-4013-bca2-2087235851a3,
            ...
  left: 1
 right: 2
```

`examples/realtime-voice/tests/adversary2_w1_key_check.rs` (1 test, `adversary2_w1_the_key_check_talk_calls_gives_up_at_key_wait`): drives `elevenlabs::resolves` over a keychain that never answers. Green at head, **8.010 s** (over 5 s, named per the budget rule). Red under the mutant of N1:

```
thread 'adversary2_w1_the_key_check_talk_calls_gives_up_at_key_wait' (2499425) panicked at examples/realtime-voice/tests/adversary2_w1_key_check.rs:45:5:
elevenlabs::resolves had not returned 10.000118339s after it was asked, over a keychain read that does not answer: talk waits before anything opens, printing nothing
```

## 3. Suite runs (restored tree, head plus the two case files; `<scratch>/lane-*.log`)

Each as `CARGO_INCREMENTAL=0 cargo nextest run <args> --no-fail-fast` through the pool:

| args | Summary line | exit | wall |
|---|---|---|---|
| `-p inference-testkit --all-features` | `Summary [  10.950s] 404 tests run: 404 passed, 0 skipped` | 0 | 14 s |
| `-p inference-openai --all-features` | `Summary [   4.796s] 349 tests run: 348 passed, 1 failed, 7 skipped` | 100 | 12 s |
| `-p inference-deepgram --all-features` | `Summary [   3.621s] 126 tests run: 126 passed, 3 skipped` | 0 | 10 s |
| `-p repo-podcast` | `Summary [   3.294s] 56 tests run: 56 passed, 0 skipped` | 0 | 8 s |
| `-p realtime-voice` | `Summary [   8.020s] 98 tests run: 98 passed, 1 skipped` | 0 | 33 s (22 s build) |

The one failure is my own 300 ms case. No test near 60 s; no lane near 120 s. Slowest: testkit `adversary_usage_view::the_suite_rejects_a_view_that_reads_an_unknown_quantity_as_zero` 10.832 s and `conform::the_suite_runs_against_the_testkit_target` 9.019 s on this build node (forge run 45451445968 at this head: about 3.8 s); openai `adversary16_live_close_after_a_stalled_write_keeps_its_documented_bound` 2.660 s; deepgram 3.011 s; repo-podcast `every_small_terminal_size_draws` 3.294 s; realtime-voice my 8.010 s case, then 2.244 s.

Mutant runs (`<scratch>/m{1,2,3}-*.log`), existing tests only:

| mutant (temporary) | existing tests that turned red |
|---|---|
| openai close: request and each read get a fresh bound (`live.rs` ~1068) | `gpt_live::close_gives_up_at_its_bound_across_every_read` 5.0 s, `adversary3_malformed_live::a_close_answered_only_by_undecodable_messages_returns` 7.0 s, `adversary16_...keeps_its_documented_bound` 7.7 s, `gpt_live_duplex::a_close_that_reaches_its_bound_keeps_the_usage_seen` 15.1 s |
| close bound ignores the setting (`live.rs:1063` `.max(CLOSE_TIMEOUT)`) | the same four |
| model's close timeout not passed to the client (`live_duplex.rs:505`) | `a_close_that_reaches_its_bound_keeps_the_usage_seen`, `a_close_answered_only_by_undecodable_messages_returns` |
| `duplex.rs:839` `>=` for `>` | all 4 of `adversary8_realtime_cancel_orders` |
| `duplex.rs:838` cancel flag not cleared | `a_cancel_is_judged_once_at_the_next_response_created`, `of_two_cancelled_responses_only_the_one_an_unreadable_message_followed_is_marked` |
| `duplex.rs:901` `undecodable_at_creation: 0` | `an_unreadable_message_before_creation_does_not_mark_a_cancelled_response` |
| `live_duplex.rs:1294` undecodable message fails the session | `an_undecodable_message_does_not_end_the_session`, `an_undecodable_message_is_counted_on_the_session`, `a_failure_after_an_undecodable_message_keeps_its_kind_and_names_it` |
| `live_duplex.rs:799` same, in the no-wait drain | `backend_usage_after_an_undecodable_message_is_recorded_by_the_close` |
| `live_duplex.rs:1138` id-less backend `Completed` dropped | `backend_usage_of_a_completed_event_without_an_id_is_counted` (2.1 s, by the two-second way out), `adversary8_live_usage_fields::a_backend_end_without_a_response_object_counts_as_unreadable` |
| deepgram `agent_duplex.rs:541` socket forgotten instead of closed | `no_agent_session_leaves_its_thread_or_connection_behind`: `connections left open: [3, 4, 8, 9, 13, 14, 18, 19]` |
| repo-podcast `show.rs:491` `>= silent_turn * 100` | `a_model_that_creates_its_own_responses_and_stays_silent_is_given_up_within_ten_seconds` 2.0 s |
| realtime-voice `resolves_within` ignores a shorter `wait` (`elevenlabs.rs:202`) | `a_keychain_that_never_answers_does_not_hold_the_key_check_forever` 3.0 s |
| realtime-voice `resolves` calls `resolves_within(source, 3600 s)` (`elevenlabs.rs:176`) | **none**: `112 tests run: 111 passed, 1 failed`, the one failure is my added case |

## 4. Findings

| id | file:line | verdict | origin | measured | what reaches it |
|---|---|---|---|---|---|
| N1 | `examples/realtime-voice/tests/adversary2_u3_talk.rs:63` | NEEDS-CHANGE | introduced | The test now drives `resolves_within(.., 1 s)` and pins the constant (`assert_eq!(elevenlabs::KEY_WAIT, 8 s)`). No test calls `elevenlabs::resolves` over a source that does not answer. Mutant `resolves` -> `resolves_within(source, 3600 s)`: 111 of 111 existing tests green (`--all-features`). Before the correction this test drove `resolves` and would have gone red. | `examples/realtime-voice/src/compare.rs:1002` (`hosted_tts`, the check `talk` and `compare` run before anything opens) and `elevenlabs::key` (`elevenlabs.rs:113`). A regression of the one-line link holds `talk` on a locked keychain. My case closes it at a cost of 8.0 s; a cheaper form needs a seam the implementor chooses. |
| N2 | `adapters/inference-openai/tests/adversary8_realtime_cancel_orders.rs:228` (same helper shape: `duplex.rs:251`, `adversary2_u1_live_boundary.rs:240`, `gpt_live_duplex.rs:284`, `adversary3_malformed_live.rs:228`) | INFEASIBLE | introduced | After the marker the read lasts at most the rest of one 100 ms wait, where it lasted 2 s. A message the server sends 300 ms after the marker is not read (red case above; the old read sees it, control green). | Nothing found: in all eleven tests the marker is the script's last message, the binding reads in order so every earlier message is processed first, queued events are drained, and no client timer under the 5 s idle limit fires after it. Only a later script that sends after its marker would lose events silently. |
| F3 | `crates/inference-testkit/tests/suite/mod.rs:325` (pass 1: `:212`) | CONFIRMED | introduced | Carried, reduced 3 -> 2. A run of the testkit's tests calls synthesis twice (authored suite, witnessed suite); the case was rewritten to assert `(2, 2)` and passes. The acceptance text is unchanged: `.engineering/planning/story/test-run-step-under-budget.md:39-40` "calls synthesis once, or never when the committed file's digest matches". | Every cold run of the testkit lane. Decision is the planner's: reword the acceptance to "once per distinct suite", or serve the authored suite from the witnessed one (the implementor's own case states every authored scenario is the same scenario in it). |
| F2r | `Taskfile.yml:35` | INFEASIBLE | introduced | Residual of pass 1's F2. At the kill (137) the bound line is now printed, the slowest list is not, and the step ends at 130 s, not the guide's 120 s (`website/docs/guides/run-the-checks.md:41`). On uutils `timeout` a part that ignores TERM lives on after the step; the guide does not say so. | Nothing in the tree ignores TERM: `task` cancels on TERM and cargo-nextest ends its tests (seen on the forge at pass 1's head: slowest list and bound line printed). uutils cannot be run here. |

Pass 1's findings:
- F1 (guide said 124, task exits 201): **resolved**. Guide line 42 says "`task test-run` exits 201"; `adversary_w1_the_test_run_task_exits_201_and_names_its_bound_when_the_bound_ends_it` asserts 201, the line, and that the guide has no "exit status 124". The commit message of `34e8ac136` still says 124.
- F2 (137 path printed no line): **resolved** for the line (`..._names_its_bound_when_a_part_outlives_the_first_signal` asserts the line, "exit status 137" and 201; green here on GNU and on the forge on uutils). Residual: F2r above.
- F3: **carried**, see table.
- F4 (empty `suite.json` served): **resolved**. `sync_all` before the rename, an empty file counts as absent; `adversary_w1_an_empty_suite_file_is_not_served_as_the_suite` and `a_suite_file_without_a_byte_is_synthesized_again` green.
- F5 (scan missed public `suite::Ess`): **resolved**. `Ess` is private; `adversary_w1_no_test_names_the_ess_synthesizer_but_the_shared_cache` green. Residuals not reached at head: an argument assembled by `concat!` (only the case file itself does it, to avoid matching itself), and a shell-string form such as `sh -c "ess verify conform synthesize ..."`, which the `"synthesize"` literal scan does not match; no test file contains either.
- F6 (stale slowest list): **resolved**. `rm -f target/nextest/default/junit.xml` before the run, `no test ran` without a report; both slowest-list cases green.
- Residual "SIGKILL from outside prints the bound line": not reachable by a test. `task` exits 201 for any failed command, so `timeout` returns 137 only when its own `task` child is killed from outside by an operator, or at its own kill-after.

## 5. Attacked, not broken

- Round 1 openai close (4 tests): three different close mutants, all four tests red each time (table above). Default `CLOSE_TIMEOUT` 15 s still documented and pinned through the constructor (`a_model_keeps_the_documented_close_timeout_unless_given_another`).
- Round 1 deepgram stalled open: forget-the-socket mutant red; stalled rounds still counted (two connections per round left open in the red output).
- Round 1 repo-podcast `Settings::silent_turn`: mutant red; default 10 s pinned through `Settings::default()`, which the loop reads at `show.rs:490`; test bounds the give-up on both sides (within 500 ms).
- Round 1 realtime-voice `resolves_within`: wait-ignored mutant red. Only the `resolves` link is open (N1).
- Short test values (2 s, 1 s, 200 ms) appear only under `tests/`.
- Zero and overflow on the new public items, read from code, not run: `with_close_timeout(ZERO)` leaves the read timeout governing (documented "the longer of"); `Duration::MAX` gives no deadline through `checked_add`, as `Endpoint::with_read_timeout(MAX)` already does; `silent_turn` ZERO gives up at the first step, MAX never; `resolves_within` ZERO/MAX go through tokio's timeout.
- Round 2 `suite::cut` (four close-rule tests): every mutant (None, FirstOnly, Restamp, Watermark, CrossSession; restamp true/false) run against the whole suite and against the cut; the statuses of the scenarios the tests read are identical. No verdict differs. Outside the cut, `inference.duplex.CloseSpeechSession/outcome/closed` and `FailSpeechSession/outcome/failed` also reject FirstOnly and CrossSession; the four tests never read them, before or after.
- Round 4 (65 `target::` tests): 65 `#[test]` before under `tests/target/`, 65 after under `tests/target_self/`; 528 `assert` lines before and after in the sorted line multiset; 25 lines gone and 42 new, all `#[cfg(test)]`, `use`, visibility or module docs. Test names: 1405 before, 404 at head; gone 1066 = 16 x 65 + 2 x 13, new 65, all in `inference-testkit::target_self`. The 13 are `stream::tests::...` formerly also run by path in `adversary2_stream_deferred` and `adversary3_target_sequence`, the same tests as among the 65. `target_self.rs` includes `#[path = "target/mod.rs"]`, the code the 15 other binaries use.
- Round 5 (bound cases read files): no case can pass with nothing printed (each asserts a printed line or the slowest-list header besides the status); the 137 case asserts "exit status 137", so it cannot pass by the 124 path. Not measured: these cases cut kill-after to 1 s, so `task` has 1 s after TERM to print the deferred list; under heavy load that window could be missed. Load tests are barred; here the case ran 1.0 s and the 137 case 2.086 s.
- Round 6 (eleven tests): besides the mutants above, the loop in `adversary2_u1_live_boundary::run` ends at three completions or the marker, a superset of the old read up to the marker.

## 6. Paths written outside the worktree

`<scratch>/` = the assigned directory `wave1/adversary-2`: `build0.log`, `build1.log`, `sherpa.log`, `run-own.sh`, `own-start`, `own-end`, `own-testkit.log`, `own-openai.log`, `own-realtime-voice.log`, `run-m1.sh`, `run-m2.sh`, `run-lanes.sh`, `m1.diff`, `m2.diff`, `m3.diff`, `m1-openai.log`, `m1-deepgram.log`, `m1-repo-podcast.log`, `m1-realtime-voice.log`, `m2-openai.log`, `m2-realtime-voice.log`, `m3-openai.log`, `lanes-status.txt`, `lane-{testkit,openai,deepgram,repo-podcast,realtime-voice}.{log,start,end}`, `r2/` (`make-copies.sh`, `tail-dcm.rs`, `tail-a11.rs`, `tail-a12.rs`), `r4/` (`extract.sh`, `names.sh`, `before-*.rs`, `before-all.txt`, `after-self.txt`, `after-target.txt`, `lines-*.txt`, `list-head.txt`, `list-head.err`, `before.sorted`, `head.sorted`, `gone.txt`, `new.txt`, `gone-names.txt`, `gone-names-uniq.txt`, `head-names-uniq.txt`, `new-names.txt`).

On the build node: the pool jobs' `target/` for this worktree, including the sherpa-onnx archive from one `slot task sherpa-onnx-archive` (the brief allows it; realtime-voice does not build without it) and `target/tmp/conformance-suite`.

For the coordinator:
1. F3: reword the acceptance or get to one synthesis.
2. N1: accept the 8 s case, ask for a cheaper seam, or accept the gap.
3. `CHANGELOG.md` `## [Unreleased]` is empty at head; the lines for the new public items (`LiveClient::with_close_timeout`/`close_timeout`, `OpenAiLive::with_close_timeout`/`close_timeout`, `Settings::silent_turn`, `elevenlabs::resolves_within`, the Taskfile behaviour) sit in `<scratch>/../u1/changelog.md` and must land with the merge.
4. Not attackable here: acceptance line 3 (no testkit test over 60 s at load above 120; load tests are barred) and uutils `timeout` behaviour beyond what the forge run shows.

Severity chosen for the two entries the store refused as `low`: `warning` for N1 and `warning` for F3.

```findings
- file: examples/realtime-voice/tests/adversary2_u3_talk.rs
  line: 63
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "The key-check test now drives resolves_within(1 s) and pins KEY_WAIT as a constant; no test drives elevenlabs::resolves over a source that does not answer. Mutant resolves -> resolves_within(source, 3600 s) leaves all 111 existing realtime-voice tests green; reached by compare.rs:1002 and elevenlabs::key. Case examples/realtime-voice/tests/adversary2_w1_key_check.rs catches it at 8.0 s."
- file: adapters/inference-openai/tests/adversary8_realtime_cancel_orders.rs
  line: 228
  category: weakened-check
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: "The read to the marker lasts at most 100 ms after the script's last message, where it lasted 2 s: a message sent 300 ms after the marker is not read (adversary2_w1_script_end.rs red, old read green). No script sends after its marker and no client timer under 5 s fires, so nothing reaches it today."
- file: crates/inference-testkit/tests/suite/mod.rs
  line: 325
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: "Carried from pass 1 (F3), reduced from three to two: a run of the testkit's tests calls synthesis twice (authored and witnessed suite); the story's acceptance still says once (.engineering/planning/story/test-run-step-under-budget.md:39-40). Planner's decision."
- file: Taskfile.yml
  line: 35
  category: docs
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: "Residual of pass 1 F2: when the kill ends the step (137) the bound line is printed but the slowest list is not, the step ends at 130 s, and on uutils timeout a part ignoring TERM outlives the step; the guide says none of this. Nothing in the tree ignores TERM; uutils not measurable here."
```
