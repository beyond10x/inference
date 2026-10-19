---
format: aep.planning-md/3
id: review-result:adversary-native-speech-contract-pass-1
kind: review-result
status: active
title: Adversary, story:native-speech-contract, pass 1
relations:
- reviews: story:native-speech-contract
revision: 1
---
```
unit: story:native-speech-contract, uncommitted working tree on base cdf2586 (worktree inference-impl-native-speech-contract)
verdict: NEEDS-CHANGE
cases: executed 665→677, red 6
origin: introduced 12 / pre-existing 0 / undecided 0
wrote-outside-worktree: 31 paths in one scratch directory (part 6)
needs-coordinator: whether a closed or failed session should end its responses in progress. This is an ESS decision (duplex.yaml) as well as a ledger one.
```

The baseline count 665 comes from the gate runs in part 3, with my two test files left out: inference-duplex 12 and inference-testkit 653.

**1. Diff stat.** `git --no-pager diff --stat` lists the same 14 tracked files as at handoff. All of them are the implementor's or the coordinator's. My additions are two untracked test files, and nothing else in the tree changed:
- `crates/inference-duplex/tests/adversary1_ledger.rs` (11 cases)
- `crates/inference-testkit/tests/adversary1_mulaw_silence.rs` (1 case, `#![cfg(feature = "tts")]`)

**2. Cases added.** Each was run alone before any suite run. Logs: `red-ledger.log` and `red-mulaw.log` in the scratch directory.

| Case (file:line) | Asserts | Now |
|---|---|---|
| adversary1_ledger.rs:120 `a_response_id_already_used_is_not_created_again` | `response_created` with an id already used is refused | red |
| :136 `a_reused_response_id_leaves_a_response_in_progress_that_takes_no_output` | after reuse, `in_progress()==Some(id)`, yet `output_audio(id)` returns `Err(ResponseNotInProgress{current: Completed})` | red |
| :154 `a_call_id_already_proposed_is_not_proposed_again` | a second `tool_call` under `call_1` is refused | red |
| :190 `the_bound_runs_once_the_providers_turn_detection_ends_a_turn_with_auto_response_on` | with server_vad and auto_response on, after `input_committed` a provider silent for 3 bounds fails the session | red |
| :251 `a_session_closed_mid_response_takes_no_more_output_of_it` | `output_audio` after `close()` is refused, and nothing follows `SpeechSessionClosed` | red |
| :204, :217, :232, :272, :311, :330 | guard cases: request after a long idle, an earlier instant heard late, idle again after the response, truncate at 0 / exactly emitted / 641, late transcript refused, ResponseFailed recorded | green |
| adversary1_mulaw_silence.rs:16 | the mock synthesiser's documented silence is mu-law silence (0xFF or 0x7F) | red |

Red output, verbatim excerpts:
```
panicked at crates/inference-duplex/tests/adversary1_ledger.rs:179:5:
auto_response is on and the provider ended the turn: the caller awaits its response, and a provider silent for three bounds leaves the session Open
panicked at ...:140:5: the ledger reports 570d3498-... in progress and refuses its audio
  left: Err(ResponseNotInProgress(ResponseStateConflict { current: Completed }))
panicked at ...:162:5: a second call under `call_1` was proposed; answer() can reach only the first
panicked at ...:122:5: ResponseCreated says `a new id`; a second response under 2826544b-... was recorded
panicked at ...:245:5: output of a closed session was recorded after SpeechSessionClosed (Ok(())): [...]
test result: FAILED. 6 passed; 5 failed
panicked at crates/inference-testkit/tests/adversary1_mulaw_silence.rs:49:17:
IncrementalStreaming: a frame of silence carries mu-law bytes [00, 00, 00, 00], which decode to about -32124
```
The red line numbers predate rustfmt on my file. The test names are unchanged.

**Mutants.** All seven ran in one scratch copy, with ledger.rs restored between them. The copy was `cargo clean`ed (639.7 MiB) and removed. Every mutant left the unit's own suite green: inference-duplex ledger 10/10 and telephony 2/2, testkit mock 7/7. Each one failed the guard case named here:

| Mutant (ledger.rs) | Caught by |
|---|---|
| :276 `>` → `>=` | :272 |
| :485 `+=` → `=` | :272 |
| :232-234 drop `heard` in request_response | :204 |
| :44 `last_heard = now` (no max) | :217 |
| :463 drop `requested = false` | :232 |
| :505 `index_of` in place of `in_progress_index` | :311 |
| :553 drop the ResponseFailed push | :330 |

Log: `mutants.log`.

**3. Suite run** (after the cases existed; each command run on its own, not piped):
- `cargo test -p inference-duplex --all-features --no-fail-fast`: EXIT=101. lib 0; adversary1_ledger `FAILED. 6 passed; 5 failed`; ledger `ok. 10 passed`; telephony `ok. 2 passed`.
- `cargo test -p inference-testkit --all-features --no-fail-fast`: EXIT=101. 653 passed, 1 failed (adversary1_mulaw_silence). Conform stayed green.
- clippy inference-duplex: EXIT=0. clippy inference-testkit: EXIT=0. fmt --check on both: EXIT=0.

**4. Findings** (tree as above):

| # | file:line | Verdict | What was measured / what reaches it |
|---|---|---|---|
| F1 | inference-duplex/src/ledger.rs:459 | INFEASIBLE, warning | **Measured:** red :120 and :136. ESS says ResponseCreated is "under a new id", but the ledger never checks the id. **Reaches it:** no adapter yet. An adapter that keys ids off Nova's `completionId`, which stays the same through a whole completion, would hit it. **Fix:** refuse a known id. |
| F2 | ledger.rs:584 | INFEASIBLE, note | **Measured:** red :154. **Reaches it:** nothing found; the providers' call ids are unique. |
| F3 | ledger.rs:619 (`awaiting`) | NEEDS-CHANGE, warning | **Measured:** red :190. With auto_response, a turn the provider ends starts no bound. Nova always runs with auto_response, so its bound only ever runs inside a response. The story asks for "a typed stall failure instead of silence". **Reaches it:** OpenAI's server_vad flow (`input_audio_buffer.committed` maps to `input_committed`). **Fix:** set `requested` on commit when auto_response is on. |
| F4 | ledger.rs:314 and ess/domains/duplex.yaml:643 | CONFIRMED, warning | **Measured:** red :251. Close leaves the response InProgress, and its output is recorded after SpeechSessionClosed. ESS's EmitOutputAudio does not check the session either. **Reaches it:** hanging up mid-answer, with the adapter still recording in-flight provider events. The mock does neither, so this is not reached today. |
| F5 | inference-testkit/src/tts.rs:297 | INFEASIBLE, note | **Measured:** red mulaw case. The "silence" is 0x00 bytes, which is full scale in pcm_mulaw. **Reaches it:** only tests/tts.rs, which checks lengths alone. |
| F6 | inference-duplex/tests/ledger.rs | CONFIRMED, warning | **Measured:** the 7 surviving mutants above. **Reaches it:** none of them are caught by the unit's tests. |
| F7 | duplex.yaml:187 and ess/scenarios/input-during-output-continues-response.yaml | CONFIRMED, warning | **Measured:** the Q15 scenario runs against the recording store and never against an adapter. Its single CreateResponse makes "no second id" true by construction, and the "dependent" text is the test's own input. The mock's Q15 test passes because the script inserts the reply. So nothing decides an adapter's `native_duplex` claim. **Reaches it:** the story's Q15 statement. |
| F8 | inference-duplex/src/contract.rs:297 | CONFIRMED, warning | **Measured:** Nova has no client cancel or truncate event: the Nova client session has only send_audio / end_audio / send_tool_result / close, and the barge-in doc says interruption is server-side only. The capabilities have no cancellation flag and `cancel_response` has no Unsupported error. A Nova adapter can only drop audio locally while the provider keeps the unheard text in its context. **Reaches it:** any Nova adapter. |
| F9 | inference-duplex/src/lib.rs:160 | CONFIRMED, note | **Measured:** ResponseUsage does not say whether it is a delta or a running total, and the ledger accepts any number of usage events per response. Nova sends `usageEvent` repeatedly with both delta and total, so a meter can double count. |
| F10 | contract.rs:29 | CONFIRMED, note | **Measured:** some provider behaviour the contract cannot express: VAD tuning (OpenAI `eagerness`, `silence_duration_ms`; Nova `endpointingSensitivity`) and G.711 A-law (OpenAI `audio/pcma`). Nova takes no mu-law (LPCM 8/16/24 kHz only), which admit refuses correctly. |
| F11 | ess/domains/audio.yaml:56-57 | CONFIRMED, note | **Measured:** a garbled comment: "…is this specification's rule, not the specification's choice, not the design's." |
| F12 | duplex.yaml:792 | CONFIRMED, note | **Measured:** the ledger's BeyondOutput refusal is neither declared nor marked UNMAPPED on TruncateOutput, unlike its format, one-at-a-time and tool-offered rules. |

**5. Attacked and not broken:**
- The mu-law VAD decode matches G.711: 0xFF→0, 0x80→32124, bias 0x84.
- 20 ms at 8 kHz is 160 bytes (`data_len`).
- `FormatMismatch` holds both ways, and a model without mu-law is refused at open.
- The liveness bound handles time going backwards (saturating, max).
- Truncation at 0, at exactly the emitted sample, and one past it, behaves as specified.
- An unknown or already-answered call id is refused.
- Late audio and transcript of a cancelled response are refused.
- The conform target parses `pcm_mulaw`.
- The Q15 scenario fails if input during output is refused.

**6. Paths written outside the worktree**, all in `<unit scratch>/adversary-1/`:
- Scripts: `mutants.sh`, `await.sh`, `ledger.rs.orig`.
- Logs: `mutants.log`, `red-ledger.log`, `red-mulaw.log`, `fmt-duplex.log`, `fmt-testkit.log`, `gate-test-duplex.log`, `gate-test-testkit.log`, `gate-clippy-duplex.log`, `gate-clippy-testkit.log`.
- Per-mutant output: `m-M{1..7}-<name>-{unit,mock,adv}.out` (21 files).

The mutant copy `scratch/adversary-1/mutant/` was cleaned and removed. The lease is released.

```findings
- {"file": "crates/inference-duplex/src/ledger.rs", "line": 459, "category": "boundary", "severity": "warning", "verdict": "INFEASIBLE", "origin": "introduced", "message": "response_created accepts an id already used, leaving a response in_progress that output_audio refuses"}
- {"file": "crates/inference-duplex/src/ledger.rs", "line": 584, "category": "boundary", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "tool_call accepts a call id already proposed, and the duplicate can never be answered"}
- {"file": "crates/inference-duplex/src/ledger.rs", "line": 619, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "with auto_response on, a turn ended by the provider starts no liveness bound, so a provider that stalls before its response is never failed"}
- {"file": "crates/inference-duplex/src/ledger.rs", "line": 314, "category": "property", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "close mid-response leaves the response InProgress and records its later output after SpeechSessionClosed"}
- {"file": "crates/inference-testkit/src/tts.rs", "line": 297, "category": "property", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "the mock synthesiser's silence is 0x00 bytes, full scale in pcm_mulaw"}
- {"file": "crates/inference-duplex/tests/ledger.rs", "category": "mutant", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "seven ledger mutants (truncate bound, emitted sum, request liveness, heard max, requested reset, transcript state, ResponseFailed event) survive the unit's suite"}
- {"file": "ess/domains/duplex.yaml", "line": 187, "category": "acceptance", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "the Q15 scenario runs only against the recording store and is true by construction, so it cannot decide an adapter's native_duplex claim"}
- {"file": "crates/inference-duplex/src/contract.rs", "line": 297, "category": "judgement", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "Nova Sonic has no client cancel, and the contract offers no capability flag or Unsupported path for cancel_response"}
- {"file": "crates/inference-duplex/src/lib.rs", "line": 160, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "ResponseUsage does not say whether it is a delta or a running total, while Nova reports both repeatedly per completion"}
- {"file": "crates/inference-duplex/src/contract.rs", "line": 29, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "SessionConfig cannot express VAD tuning or G.711 A-law, which both providers offer"}
- {"file": "ess/domains/audio.yaml", "line": 56, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "the edited sample_count comment is garbled"}
- {"file": "ess/domains/duplex.yaml", "line": 792, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "the BeyondOutput refusal is neither declared nor marked UNMAPPED on TruncateOutput"}
```