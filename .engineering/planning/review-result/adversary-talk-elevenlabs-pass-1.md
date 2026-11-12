---
format: aep.planning-md/3
id: review-result:adversary-talk-elevenlabs-pass-1
kind: review-result
status: active
title: Adversary pass 1 on talk picks the ElevenLabs cascade
relations:
- reviews: story:talk-picks-elevenlabs
revision: 1
---
unit: story:talk-picks-elevenlabs, `talk --provider elevenlabs-cascade` and the `compare` refactor, commit 5f7dbeaf plus one untracked test file
verdict: NEEDS-CHANGE
cases: executed 83→88, red 6 (5 new cases, 1 existing wall-clock test that fails intermittently)
origin: introduced 4 / pre-existing 1 / undecided 1
wrote-outside-worktree: 8 paths
needs-coordinator: F6 counts as pre-existing because the line is the same at base, but the story's acceptance ("`compare` and `talk` reach the host `ELEVENLABS_RESIDENCY` names") covers it. Moving it out of the wave leaves that acceptance line unmet.

**1. `git --no-pager diff --stat`**
Empty. Nothing tracked was changed. `git status --short` shows only the new untracked test file:
```
?? examples/realtime-voice/tests/adversary_u3_talk.rs
```

**2. Cases added** (`examples/realtime-voice/tests/adversary_u3_talk.rs`, all red now)

Run alone with `cargo test -p realtime-voice --test adversary_u3_talk -- --test-threads=1`, exit 101. Full log: `<scratch>/case-red.log`. The red lines, verbatim:
```
a_recognition_that_fails_before_the_end_of_input_ends_the_conversation_and_says_why ... FAILED
  the recognition failed (the host refused it) and the conversation was still open after 8.002439199s, with notes []
a_turn_committed_while_the_last_answer_still_plays_stops_that_playout ... FAILED
  a second turn was committed while 4s of the first answer's audio was still queued, and the playout was never stopped: [Heard("tell me a story"), Said(…)×8, Answered, Heard("stop and tell me the time")]
after_a_barge_in_the_synthesiser_is_offered_no_further_segment ... FAILED
  10 segments were offered to the synthesiser after the barge-in cut the answer (at [145.624004ms, 295.788928ms, 445.981978ms, 598.031795ms, 748.221994ms, 898.421575ms, 1.04875495s, 1.199006115s, 1.349208631s, 1.499410074s] after it); 12 offers in all
closing_the_conversation_while_an_answer_is_queued_returns_promptly ... FAILED
  converse returned 1.662175078s after the conversation was closed, having offered 12 segments
the_keychain_source_talk_documents_is_checked_without_a_panic ... FAILED
  panicked at …/secrets-keychain/src/lib.rs:166:9: there is no reactor running, must be called from the context of a Tokio 1.x runtime
  checking the keychain source panicked: Err("there is no reactor running, must be called from the context of a Tokio 1.x runtime")
test result: FAILED. 0 passed; 5 failed; 0 ignored; 0 measured; 0 filtered out; finished in 12.18s
```
What each case asserts:
- **Recognition failure:** the recogniser is the one `talk` builds (`elevenlabs::recogniser`), pointed at a loopback server that answers the upgrade with 403. The case asserts that `Ended` and a `Refused` note follow.
- **Turn during playback:** a turn committed while the previous answer's audio is still queued should stop the playout.
- **Barge-in:** a synthesiser that blocks 150 ms on each segment should be offered no segment after the barge-in.
- **Closing:** `converse` should return within 600 ms after `open` turns false.
- **Keychain:** `elevenlabs::resolves` on `KeychainSource::native()` should not panic and should not refuse the source for being asynchronous. The panic comes before any keychain read starts.

**3. Suite run** (after the cases existed)
`cargo test -p realtime-voice --all-features --no-fail-fast`, exit 101. Full log: `<scratch>/suite.log`.
- 88 cases executed. All pass except my 5 and one existing test: `adversary2_topology_workers.rs:494`, which reported `barge-in to stop 3.7325ms: the attack of 30ms needs two frames of 20ms sent at pace`.
- The 83 before my additions comes from the same run with my binary's 5 left out, not from a separate run.
- That existing test, rerun alone 8 times (`<scratch>/flake.log`), failed once (`7.000405ms`). It depends on wall-clock timing.

**4. Findings** (commit 5f7dbeaf)

| # | file:line | finding | verdict / origin | what reaches it |
|---|---|---|---|---|
| F1 | `examples/realtime-voice/examples/talk.rs:325` (via `src/elevenlabs.rs:56`) | Without `ELEVENLABS_API_KEY`, `talk` checks the keychain key by polling it once on a thread with no Tokio runtime. The keychain read uses `spawn_blocking`, so the check panics (exit 101). | NEEDS-CHANGE / introduced | The guide's first-listed setup: `secretsctl put elevenlabs-api-key`, then run without the variable. Fix: run the check on a runtime, or let the binding resolve the key. |
| F2 | `src/talk.rs:447` | Before input ends, an ElevenLabs recognition that has failed only yields `None`, and the usage record is refused. `converse` never ends input, so `Ended` and the `failure` note are never reached. `talk` keeps listening to a dead recogniser and says nothing. | NEEDS-CHANGE / introduced | Any mid-session failure, including the wrong-residency refusal this story exists for. Fix: check `failure` on every report, not only at usage. |
| F3 | `src/talk.rs:372` with `src/compare.rs:1140` | `cut` drops the sender, but `speaking` still offers every segment already queued. After a barge-in, or at Enter, the whole generated answer is still synthesised and billed. `answer()` and the end of `converse` block until that finishes (1.66 s measured). The module doc and the guide (`talk-to-a-hosted-voice-model.md:132`, "bills the sentence it was speaking") say only one segment. | NEEDS-CHANGE / introduced | Every barge-in or Enter while the LLM is ahead of TTS, which is normal for a local LLM. |
| F4 | `src/talk.rs:459` | A new turn stops the playout only `if self.answering`. Once the old answer is fully synthesised, its queued audio (buffer up to 120 s) keeps playing and the new answer plays after it. | CONFIRMED / introduced | Speech that ElevenLabs' voice detection commits but that stays below the local barge-in threshold (RMS 0.05). |
| F5 | `examples/talk.rs:264-265, 323, 339`; guide lines 115 and 118 | Exit-code and ordering drift. The guide says checks run in table order, but `ELEVENLABS_RESIDENCY` is checked before the key. With no keychain available, `KeychainSource::native()?` exits 1, though the guide promises 2. A non-loopback `INFERENCE_LLAMA_SERVER_URL` exits 1, though `USAGE_EXIT` covers a value "not taken". | CONFIRMED / introduced (judgement, not run: it needs the example binary) | Headless machines; mistyped values. |
| F6 | `examples/compare.rs:514, 519` | `compare`'s ElevenLabs agent arm uses `AgentEndpoint::new()` (the global host) whatever `ELEVENLABS_RESIDENCY` says. Only the cascade voice follows the variable. | NEEDS-CHANGE / pre-existing | The default `compare` provider list includes `elevenlabs`; anyone with an agent on a residency stack hits it. |
| F7 | `tests/adversary2_topology_workers.rs:494` | Wall-clock assertion that fails intermittently (2 of 9 runs here). | CONFIRMED / undecided (base not built) | The suite under load. |

**5. Attacked and could not break**
- **`compare`'s measurements:** the extraction of `Segments`, the spawn helpers and `request` keeps the same order and instants. `first_token` is still stamped on every `TextDelta`, including refused deltas and those before `Started`. The `failure` closure `compare` passes always returns `None`, so `listening` behaves as before.
- **Residency in `talk`:** the recogniser and the synthesiser share the one `endpoint()`, and `compare`'s cascade voice uses it too.
- **Frames after a cut:** they are dropped by the turn tag, and the `Idle` reports (tag 0) fall through harmlessly.
- **Missing variables:** the order is URL, model, voice; blank values count as unset; without feature `local` the exit is 2.
- **A new answer's audio after an old one:** the speaker queues it as a new segment of the clock and does not refuse it.

**6. Paths written outside the worktree**
- `<scratch>/wave-p1/U3/review-1/` (directory)
- `<scratch>/wave-p1/U3/review-1/run-case.sh`
- `<scratch>/wave-p1/U3/review-1/case.log`
- `<scratch>/wave-p1/U3/review-1/case-red.log`
- `<scratch>/wave-p1/U3/review-1/run-suite.sh`
- `<scratch>/wave-p1/U3/review-1/suite.log`
- `<scratch>/wave-p1/U3/review-1/run-flake.sh`
- `<scratch>/wave-p1/U3/review-1/flake.log`

Inside the worktree, cargo also wrote build output under `target/`.

**7. findings**
```findings
- file: examples/realtime-voice/examples/talk.rs
  line: 325
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: talk checks a keychain key by polling it once on a thread with no Tokio runtime, and the keychain read's spawn_blocking panics, so the documented keychain path exits 101
- file: examples/realtime-voice/src/talk.rs
  line: 447
  category: contract-drift
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: an ElevenLabs recognition that fails before the end of input yields only None, so converse never reports Ended or why, and talk goes on silently over a dead recogniser
- file: examples/realtime-voice/src/talk.rs
  line: 372
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a cut drops the synthesis sender but speaking still offers every queued segment, so a barge-in or Enter still synthesises and bills the generated answer, and the next answer and the end of the conversation wait for it
- file: examples/realtime-voice/src/talk.rs
  line: 459
  category: boundary
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a turn committed after the previous answer was fully synthesised does not stop that answer's queued audio, so the new answer plays only after it
- file: examples/realtime-voice/examples/talk.rs
  line: 264
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the residency is checked before the key, against the guide's table order, and an unavailable keychain or a non-loopback server URL exits 1 where the guide and USAGE_EXIT promise 2
- file: examples/realtime-voice/examples/compare.rs
  line: 519
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: pre-existing
  message: compare's ElevenLabs agent arm uses AgentEndpoint::new and ignores ELEVENLABS_RESIDENCY, against the story's acceptance that compare reaches the host the variable names
- file: examples/realtime-voice/tests/adversary2_topology_workers.rs
  line: 494
  category: concurrency
  severity: note
  verdict: CONFIRMED
  origin: undecided
  message: the wall-clock barge-in-to-stop assertion failed in 2 of 9 runs, once in the suite and once in 8 runs alone
```