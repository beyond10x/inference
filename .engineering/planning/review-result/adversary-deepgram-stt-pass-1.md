---
format: aep.planning-md/3
id: review-result:adversary-deepgram-stt-pass-1
kind: review-result
status: active
title: Adversary pass on the Deepgram streaming STT adapter, pass 1
relations:
- reviews: story:deepgram-streaming-stt-adapter
revision: 1
---
unit: 0.17.0 U2 story:deepgram-streaming-stt-adapter, tree inference-0-17-0-u2-deepgram-stt, findings cover 90d0780 (commit b413b04 adds tests only)
verdict: NEEDS-CHANGE
cases: executed 63→78, red 5 (4 new, 1 existing flaky)
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 5 (logs and a message file in the U2 scratch directory)
needs-coordinator: clippy (both crates) still waiting for the build slot; worktree discard-cache not run

**Diff** `git diff --stat 90d0780..b413b04`: `adapters/inference-deepgram/tests/adversary1_stream.rs` (+131) and `crates/inference-testkit/tests/adversary1_stt_mutants.rs` (+362). Tests only.

**Cases**
- `adversary1_stt_mutants` (8 passed, 2 failed): one broken recogniser per check.
  - Caught (green): a revision of a committed span, a swallowed event, ending before the usage record, a drain slower than its bound, a failure with no usage record, a usage record that counts no audio, incremental input claimed but frames taken only at the end.
  - Red: Cancellation passes a recogniser that replays a dropped recognition's words into the next one (`Ok(CancellationObserved { max_concurrency: 1, frames_before_drop: 80 })`). The check compares span ids only, and fresh ids are always new.
  - Red: EndOfInput cannot fail a handle that blocks; it had not answered 2 s after a 200 ms drain bound, because the bound is measured only after the drain returns.
- `adversary1_stream` (3 passed, 2 failed):
  - Red: a `Metadata` that arrives before `CloseStream` and is still queued when input ends is taken as the summary; the final result after `CloseStream` is lost. Got `["usage"]`, expected `["hypothesis late words","commit","usage"]`.
  - Red: a final result after an `UtteranceEnd` commits the same words twice. Got `["good morning","good morning"]`.
  - Green: no audio ends at once with its usage record and opens no connection; no zero-byte frame is sent; iterating inside a current-thread tokio runtime works.

**Suite** (`--no-fail-fast`): `test -p inference-deepgram --tests --all-features` rc=101: lib 14, audio 7, auth 5, cancel 2, claims_record 1, errors 8, transcripts 5 passed, live 1 ignored; adversary1_stream 3/2 failed; conformance 7/1 failed. `test -p inference-testkit --features stt --test stt_conformance --test adversary1_stt_mutants`: stt_conformance 13/0, adversary1 8/2, rc=101. The conformance failure is the unit's own `a_dropped_recognition_frees_its_slot_and_revives_nothing` (`tests/conformance.rs:74`, `left: Some(1) right: Some(0)`): 1 of 10 parallel reruns failed, 0 of 5 alone; the fresh recognition can close before the dropped one.

**Judgement**: an `Error` message is fatal although the notes call it non-fatal on v1 (documented decision, not raised). Message size bounded only by tungstenite's 64 MiB default. A final result with an empty transcript commits the earlier interim text (note below).

**Attacked, not broken**: typed failures for every close code, 401/403, `Error`, non-JSON text, a close before `Metadata`, silence after `CloseStream`; cancel before the first frame, after end of input, twice; drop closes the socket; μ-law and A-law only at 8 kHz; the claims record matches. Cargo about 0.5 min, wall about 95 min (slot wait), peak `target/` 1.8G.

**Reachability**: stale `Metadata` and the double commit were constructed; the notes do not say a server produces either. The two check weaknesses and the flaky close order are reachable.

```findings
[
{"file":"adapters/inference-deepgram/src/recogniser.rs","line":386,"category":"contract-drift","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"A Metadata received before CloseStream but still queued when input ends is taken as the summary, ending the recognition and losing the final results sent after CloseStream, contrary to the listen.rs claim that such a Metadata is ignored."},
{"file":"adapters/inference-deepgram/src/recogniser.rs","line":380,"category":"boundary","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"An UtteranceEnd commits the open interim span and a later is_final Results for the same start opens and commits a second span with the same words, because the binding ignores start and duration."},
{"file":"crates/inference-testkit/src/stt/conformance.rs","line":473,"category":"mutant","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"cancellation only compares span ids, which a fresh recognition always issues anew, so a recogniser that replays a dropped recognition's transcript into the next one passes, and for a hosted adapter the dropped span set is empty."},
{"file":"crates/inference-testkit/src/stt/conformance.rs","line":853,"category":"mutant","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"drain measures the bound only after poll returns, so a handle that blocks after end of input holds the check for as long as it blocks instead of failing it at the bound."},
{"file":"adapters/inference-deepgram/tests/conformance.rs","line":74,"category":"concurrency","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The test asserts the first client close is connection 0, but the fresh recognition can close first; it failed in the suite run and once in ten parallel reruns."},
{"file":"adapters/inference-deepgram/src/recogniser.rs","line":373,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A final Results with an empty transcript commits the open span with the earlier interim text, so text the endpoint finalized as nothing is committed."}
]
```
