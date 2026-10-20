---
format: aep.planning-md/3
id: review-result:final-review-realtime-openai
kind: review-result
status: active
title: Final adversary review, realtime-openai area
relations:
- reviews: story:openai-realtime-client
- reviews: story:openai-realtime-adapter
revision: 1
---
unit: OpenAI Realtime area (story:openai-realtime-client, story:openai-realtime-adapter), tree inference-review-realtime-openai at c733f12 plus 2 untracked test files
verdict: NEEDS-CHANGE
cases: executed 44→53, red 6
origin: introduced 12 / pre-existing 0 / undecided 0
wrote-outside-worktree: 7 paths (part 6)
needs-coordinator: none

**1. Diff stat**
`git --no-pager diff --stat` is empty: nothing tracked changed. `git status --short` shows two untracked test files and nothing else:
```
?? adapters/inference-openai/tests/adversary2_client.rs
?? adapters/inference-openai/tests/adversary2_duplex.rs
```

**2. Cases added.** These were run alone before the suite. Full log: `.../review-realtime-openai/red-run.log`. Base 8c4da2c has no `adapters/inference-openai`, so every finding is `introduced`.

| case | asserts | now | red output (verbatim, trimmed) |
|---|---|---|---|
| `a_caller_commit_under_server_vad_with_auto_response_gets_a_response` | after a caller commit under server VAD with auto_response on, the binding sends `response.create` and the session does not stall | red | `left: None right: Some(Object {"type": String("response.create")})` … `kind: Stalled, detail: "the provider sent nothing for 609 ms, the bound is 600 ms"` |
| `a_refused_caller_commit_does_not_swallow_the_next_server_commit` | a caller commit the server refuses, then a VAD commit, gives 2 `InputCommitted` | red | `assertion left == right failed: the caller's commit and the server's turn-end commit` (left 1) |
| `the_input_transcription_usage_is_recorded` | the reference's `usage` on `...transcription.completed` becomes `TranscriptionUsageRecorded{13,9}` | red | `the transcription usage the server reported is dropped: [InputTranscribed(...)]` |
| `the_live_evidence_oracle_refuses_a_session_that_never_opened` | the live test's oracle fails when the session never opened | red | `the live test's assertion passes on a session that never opened: Err(Open(Failed(SessionFailure { kind: TransportClosed, detail: "connection failed: IO error: Connection refused (os error 111)" })))` |
| `dropping_a_session_inside_an_async_context_does_not_panic` | dropping a session inside an async task does not panic | red | `Cannot drop a runtime in a context where blocking is not allowed.` |
| `a_response_done_whose_usage_lacks_the_totals_is_not_a_protocol_violation` (client file) | usage without totals decodes (the reference requires none of them) | red | `Err(Protocol("`response.done` lacks the non-negative integer field `total_tokens`"))` |
| `truncation_converts_the_heard_sample_at_the_output_rate` | guard: 80 mu-law samples → `audio_end_ms: 10` | green | red under mutant M1 |
| `speech_positions_count_at_the_input_rate` | guard: 1000/1500 ms of mu-law input → samples 8000/12000 | green | red under mutant M2 |
| `usage_without_details_and_an_incomplete_response` | guard: totals with no details count as text; `incomplete` → ResponseFailed | green | red under M3 and M4 |

**Mutants.** Applied to a scratch copy, never to the tree (log `mutants.log`). Under each one the existing `--test duplex` still passed (`test result: ok. 11 passed`) and the guard case named above went red.
- M1: duplex.rs:593 `output_format` changed to `input_format`.
- M2: duplex.rs:398 input rate changed to output rate.
- M3: duplex.rs:535 the fallback changed to `0`.
- M4: duplex.rs:503-505 `Incomplete` changed to completed.

**Key-leak probe.** The test needs `log` as a dev-dependency, which a manifest edit would add, so it ran in the scratch copy (log `probe-log.log`). With a trace-level `log` logger installed, `connect` logs the key:
`TRACE tungstenite::handshake::client: Request: "GET /v1/realtime?model=gpt-realtime-2.1 HTTP/1.1\r\n…authorization: Bearer sk-probe-secre…`
Source: tungstenite-0.30.0 `src/handshake/client.rs:197` traces the raw request bytes. `set_sensitive` does not cover that path.

**3. Suite run** (after the cases existed). Command: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p inference-openai --all-features --no-fail-fast`, EXIT=101.
```
unittests 2 passed | adversary1_realtime 12 passed | adversary2_client 0 passed; 1 failed
adversary2_duplex 3 passed; 5 failed | duplex 11 passed | live 1 ignored | live_duplex 2 ignored | offline 19 passed
error: 2 targets failed
```
- `executed <before>` = 44: the same run without my two files (2+12+11+19).
- `cargo clippy -p inference-openai --all-targets --all-features -- -D warnings`: exit 0.
- `cargo fmt -p inference-openai --check`: exit 0, after I ran rustfmt on my two files only.

**4. Findings** (tree c733f12 plus the untracked tests)

| file:line | verdict | what was measured | what reaches it |
|---|---|---|---|
| src/duplex.rs:565 | NEEDS-CHANGE | `response.create` is sent only when turn detection is Disabled. Under VAD with auto_response, the ledger awaits a response that the API never creates for a client commit (reference: "will not create a response"), and the session fails as Stalled. | The contract claims manual_commit in every mode, and the testkit mock answers this call with a response. No in-repo caller commits under VAD. |
| src/duplex.rs:463 | CONFIRMED | `caller_commits` is never decremented on an `error` reply, so the next VAD commit is taken as the acknowledgement and dropped. | A caller commit under VAD that the server refuses, e.g. a buffer under 100 ms. No in-repo caller. |
| src/duplex.rs:449 (event.rs:147) | NEEDS-CHANGE | The `usage` of `...transcription.completed` is never parsed or recorded. The ESS says transcription usage "are recorded as events", and the mock records it. | Every session with transcription on. That is the default (`DEFAULT_TRANSCRIPTION_MODEL`), used by live_duplex and talk.rs. |
| src/client.rs:96 (lib.rs:18) | NEEDS-CHANGE | lib.rs says the key "is not … logged", but a trace logger gets it in clear text. | Any embedder with a trace-level `log` logger, or tracing's log bridge. Nothing in the repo installs one. Fix options: correct the doc, or cap `log` levels for tungstenite. |
| tests/live_duplex.rs:211 | NEEDS-CHANGE | The oracle passes on `Err(Open(_))`: a refused handshake, a wrong key or no network passes with no exchange. | `cargo test --features live -- --ignored` with a bad env key. Fix: require `!matches!(outcome, Err(NoNativeDuplex::Open(_) \| NoNativeDuplex::Failed(_)))`. |
| src/duplex.rs:324 | INFEASIBLE | The session owns a Runtime, and dropping it inside an async task panics. The module doc forbids only "driving" it there. | Nothing found: no async consumer in the repo. |
| src/event.rs:288 | INFEASIBLE | Usage totals are required although the reference requires none. A refusal fails the whole duplex session as ProviderError. | Nothing found: no evidence the server omits them. |
| src/duplex.rs:593 | CONFIRMED | Mutant M1 survives the suite (truncation tested only with 24 kHz in and out). | Guard case added. |
| src/duplex.rs:398 | CONFIRMED | Mutant M2 survives (speech positions tested only with 24 kHz in and out). | Guard case added. |
| src/duplex.rs:535 | CONFIRMED | Mutant M3 survives (usage without details is untested). | Guard case added. |
| src/duplex.rs:503 | CONFIRMED | Mutant M4 survives (`incomplete`/`failed` statuses are untested). | Guard case added. |
| src/duplex.rs:306 | CONFIRMED | Judgement: an HTTP 401 handshake refusal is `Connect`, which maps to TransportClosed, not Rejected. | Any wrong key at `open`. No case written. |

**5. Attacked, could not break**
- Wire names checked against the OpenAPI reference: `audio/pcm` + rate 24000, `audio/pcmu`, `audio/pcma` (`RealtimeAudioFormats`). Whether `audio/pcma` works live is still unchecked.
- Typed server events: their required fields match the reference.
- Key leaks other than the log: handshake errors (Display/Debug) and the Debug output of `Credential`, `EnvironmentSource`, `OpenAiRealtime` and `RealtimeClient` do not carry the key.
- Provider response ids: cancel uses the mapped id, and late audio after a cancel is dropped.
- A caller cancel followed by the server's cancelled `response.done` is not recorded twice.
- The outer `timeout` around `next_event` is cancel-safe (code reading only).
- Resampler bounds hold.

**6. Paths written outside the worktree**
- `<host path>`
- `<host path>`
- `<host path>`
- `<host path>`
- `<host path>`
- `<host path>`
- `<host path>`
- `<host path>`

The mutant copy `.../review-realtime-openai/mutant/` (with its target/) was cargo-cleaned and removed. The lease is released.

**7. Findings block**
```findings
- {"file": "adapters/inference-openai/src/duplex.rs", "line": 565, "category": "contract-drift", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "a caller commit under server or semantic VAD with auto_response sends no response.create, the API creates none for a client commit, and the ledger fails the healthy session as Stalled"}
- {"file": "adapters/inference-openai/src/duplex.rs", "line": 463, "category": "concurrency", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "a caller commit the server refuses keeps caller_commits raised, so the next VAD commit is swallowed and no InputCommitted is recorded"}
- {"file": "adapters/inference-openai/src/duplex.rs", "line": 449, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "the required usage of input_audio_transcription.completed is never parsed or recorded as TranscriptionUsageRecorded"}
- {"file": "adapters/inference-openai/src/client.rs", "line": 96, "category": "contract-drift", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "tungstenite traces the raw handshake request, so a trace-level log logger receives the bearer key the crate doc says is never logged"}
- {"file": "adapters/inference-openai/tests/live_duplex.rs", "line": 211, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "the native_duplex live oracle passes when the session never opens, so the live test passes with no exchange with the hosted model"}
- {"file": "adapters/inference-openai/src/duplex.rs", "line": 324, "category": "judgement", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "the session-owned runtime panics when the session is dropped inside an async task, which the module doc does not forbid"}
- {"file": "adapters/inference-openai/src/event.rs", "line": 288, "category": "contract-drift", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "usage totals the reference does not require are decoded as required, and a refusal fails the whole duplex session"}
- {"file": "adapters/inference-openai/src/duplex.rs", "line": 593, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "converting the truncation at the input rate survives the suite"}
- {"file": "adapters/inference-openai/src/duplex.rs", "line": 398, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "converting speech positions at the output rate survives the suite"}
- {"file": "adapters/inference-openai/src/duplex.rs", "line": 535, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "zeroing the text-token fallback for usage without details survives the suite"}
- {"file": "adapters/inference-openai/src/duplex.rs", "line": 503, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "mapping an incomplete response to completed survives the suite"}
- {"file": "adapters/inference-openai/src/duplex.rs", "line": 306, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "a handshake the server refuses for the key (HTTP 401) is reported as TransportClosed, not Rejected"}
```