---
format: aep.planning-md/3
id: review-result:adversary-talk-elevenlabs-pass-2
kind: review-result
status: active
title: Adversary pass 2 on talk picks the ElevenLabs cascade
relations:
- reviews: story:talk-picks-elevenlabs
revision: 1
---
unit: story:talk-picks-elevenlabs, branch impl/talk-picks-elevenlabs at 5545acb2, plus one untracked test file
verdict: CONFIRMED
cases: executed 88→91, red 2
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 13 paths, all under `<scratch>/wave-p1/U3/review-2/`
needs-coordinator: none

**1. `git --no-pager diff --stat`**
Empty: no tracked file changed. `git status --short` shows one untracked file:
```
?? examples/realtime-voice/tests/adversary2_u3_talk.rs
```

**2. Cases added** in `examples/realtime-voice/tests/adversary2_u3_talk.rs`, run alone with `slot cargo test -p realtime-voice --all-features --test adversary2_u3_talk -- --test-threads=1`, exit 101. This was the first time anything ran them. Log: `<scratch>/wave-p1/U3/review-2/case.log`.
```
test a_cut_synthesis_reads_no_further_event_of_the_segment_it_speaks ... ok
test a_keychain_that_never_answers_does_not_hold_the_key_check_forever ... FAILED
test a_server_url_not_taken_is_named_before_a_later_variable_that_is_unset ... FAILED
---- a_keychain_that_never_answers_does_not_hold_the_key_check_forever stdout ----
panicked at examples/realtime-voice/tests/adversary2_u3_talk.rs:66:5:
elevenlabs::resolves had not returned 10.001026652s after it was asked, over a keychain read that does not answer: talk waits before anything opens, printing nothing
---- a_server_url_not_taken_is_named_before_a_later_variable_that_is_unset stdout ----
panicked at examples/realtime-voice/tests/adversary2_u3_talk.rs:100:5:
the server URL, first in the guide's table, is not loopback, and talk names another variable first: ELEVENLABS_VOICE_ID is not set
test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10.11s
exit 101
```
- **Keychain that never answers (red).** The test key source has the same shape as the keychain backend: the future awaits `tokio::task::spawn_blocking` around a read that does not come back. The case asserts that `elevenlabs::resolves` returns within 10 s. I chose the 10 s bound.
- **Check order (red, needs feature `local`).** The server URL is `http://192.0.2.1:8080/v1`, which is not loopback, the model is set and the voice is unset. The case asserts that the refusal names `INFERENCE_LLAMA_SERVER_URL`, because the guide and the changelog say the variables are checked in table order.
- **Cut while a segment streams (green now; it catches a mutant).** One segment of 40 words, and each event is read 50 ms after it is asked for. The conversation is closed at the first played frame. The case asserts at most 2 further reads and a return within 400 ms.
  - The mutant, in a scratch copy (`<scratch>/wave-p1/U3/review-2/mutant.diff`), deletes the cut-flag check after each event at `src/compare.rs:1179-1181`. The case fails on it with `the cut synthesis read 40 more events after the conversation was closed, and converse returned 2.05630928s after it` (`mutant.log`).
  - Every existing test file (88 cases, `--all-features`) passes on the same mutant (`mutant-suite.log`, exit 0). So no existing test covers that check.

**3. Suite run**, made after the cases existed: `slot cargo test -p realtime-voice --all-features --no-fail-fast`, exit 101. Log: `<scratch>/wave-p1/U3/review-2/suite.log`.
- 91 cases executed. Only my two red cases fail.
- The 88 "before" is the same run with my three cases left out (91 − 3). The run against the mutant copy, which excluded my file, also counted 88.
- All 5 pass-1 cases (`adversary_u3_talk.rs`) pass. `compare.rs` 5/5, `topology_latency.rs` 5/5 and `adversary2_topology_workers.rs` 4/4 pass.

**4. Findings** (commit 5545acb2)

| # | file:line | finding | verdict / origin | what reaches it |
|---|---|---|---|---|
| G1 | `examples/realtime-voice/src/elevenlabs.rs:65` | `resolves` waits on the key source with `block_on` and no time limit. A keychain that never answers holds `talk` before its first line of output, with nothing printed and Enter not yet read; only Ctrl-C ends it. The guide promises exit 2 when the keychain "does not open or gives no key". A time limit on `block_on` alone would not fix it: dropping the runtime still waits for the blocking read, so it also needs `shutdown_background`/`shutdown_timeout`, or a "waiting for the keychain" line printed first. | CONFIRMED / introduced | Without `ELEVENLABS_API_KEY`, the Secret Service store calls `ensure_unlocked`, and that waits on an unlock prompt with no timeout. It is reached when nobody answers that prompt (for example, it is shown on another display). The case only builds the never-answering read; I did not show that someone actually leaves a prompt unanswered. |
| G2 | `examples/realtime-voice/src/talk.rs:137` (`cascade_required`), with the guide's table and `CHANGELOG.md` | The changelog names `Provider::required` as checking "in the order the guide lists them", including values not taken. It only checks that the three variables are set. The URL's loopback check runs later, in `examples/talk.rs` `generator()`. A non-loopback URL with an unset voice (or an unset model) is reported as the later variable; the URL is named only on the next run. Both exit 2. | CONFIRMED / introduced | Any user with a mistyped server host and an incomplete environment. Fix: check loopback inside `cascade_required`, or say in the guide that presence is checked before values. |
| G3 | `examples/realtime-voice/src/compare.rs:1179` | No existing test covers the after-each-event cut check in `speaking`. Without it, a cut synthesis reads the rest of the segment it is speaking (measured 2.06 s) before `answer()`'s join or the end of `converse` returns. My case covers it. | CONFIRMED / introduced | Every barge-in or Enter while ElevenLabs streams a segment; its `next()` blocks on each socket read. |

**5. Attacked and could not break**
- **F1:** the keychain source no longer panics. The pass-1 case is green, and the check now runs on its own runtime on a scoped thread.
- **F2:** `Heard::Failed` is reported once, after every drain. A recognition refused at the upgrade ends the conversation with the reason. Before input ends, the ElevenLabs `usage()` returns `Err`, so a failed recognition cannot loop.
- **F3:** no segment is offered after a cut. The cut worker ends after the event it is reading, and the queued tasks' `Owed` counts are released when it ends.
- **F4:** `answer` stops the playout whenever `playing()` is true. A committed empty transcript does not stop it.
- **F5:**
  - Order is server URL, model, voice, key, residency.
  - A keychain that does not open, a non-loopback URL, an adapter refusal and an invalid voice id all exit 2.
  - The recogniser and synthesiser constructors still exit 1, but they cannot fail once the earlier checks have passed.
- **`compare`:** it passes a flag it never sets, and its failure closure returns `None`, so `speaking` and `listening` behave as before. The fix round's compare diff is only the flag and `Heard::Failed`, and compare's measurement tests are green.
- **Cut flag in `Heard::Synthesised`:** setting the flag of a worker that has already finished is harmless.

**6. Paths written outside the worktree**
- `<scratch>/wave-p1/U3/review-2/` (directory)
- `<scratch>/wave-p1/U3/review-2/run-case.sh`
- `<scratch>/wave-p1/U3/review-2/case.log`
- `<scratch>/wave-p1/U3/review-2/setup-mutant.sh`
- `<scratch>/wave-p1/U3/review-2/mutant/` (a mutated copy of the `realtime-voice` package: `Cargo.toml`, `Cargo.lock`, `src/`, `tests/`)
- `<scratch>/wave-p1/U3/review-2/mutant.diff`
- `<scratch>/wave-p1/U3/review-2/run-mutant.sh`
- `<scratch>/wave-p1/U3/review-2/mutant.log`
- `<scratch>/wave-p1/U3/review-2/run-mutant-suite.sh`
- `<scratch>/wave-p1/U3/review-2/mutant-suite.log`
- `<scratch>/wave-p1/U3/review-2/adapters` (a symlink to the worktree's `adapters/`, so the copied tests find their fixtures)
- `<scratch>/wave-p1/U3/review-2/run-suite.sh`
- `<scratch>/wave-p1/U3/review-2/suite.log`

The mutant copy was built into the worktree's own `target/`, so the build output stayed inside the worktree.

**7. findings**
```findings
- file: examples/realtime-voice/src/elevenlabs.rs
  line: 65
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: resolves waits on the key source with no time limit, so a keychain read that never answers holds talk silently before anything opens, where the guide promises exit 2, and a time limit on block_on alone would still hang when the runtime is dropped
- file: examples/realtime-voice/src/talk.rs
  line: 137
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Provider::required, which the changelog says checks in the guide's table order including values not taken, only checks that variables are set, so a non-loopback server URL is reported after a later unset variable
- file: examples/realtime-voice/src/compare.rs
  line: 1179
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: no existing test covers the cut check after each event in speaking; deleting it lets a cut synthesis read the rest of its segment (2.06 s measured) with all 88 existing cases green
```