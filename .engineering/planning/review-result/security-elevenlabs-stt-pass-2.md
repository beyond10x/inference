---
format: aep.planning-md/3
id: review-result:security-elevenlabs-stt-pass-2
kind: review-result
status: active
title: Security review of the ElevenLabs realtime STT adapter, pass 2
relations:
- reviews: story:elevenlabs-realtime-stt-adapter
revision: 2
---
```
unit: story:elevenlabs-realtime-stt-adapter, commit fc0b0b0d (base 1644e13f) plus two uncommitted test files
verdict: NEEDS-CHANGE
cases: executed 140→145, red 3
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 4 paths (the security2 scratch directory and 3 files in it)
needs-coordinator: no
```

**1. Diff proof**

`git --no-pager diff --stat` is empty: no tracked file changed. `git status --short` shows two new files, both tests:
```
?? adapters/inference-elevenlabs/tests/security2_u1_end_deadline.rs
?? adapters/inference-elevenlabs/tests/security2_u1_opening.rs
```

**2. Cases added**

I ran the two files alone, before the suite: `cargo test -p inference-elevenlabs --all-features --test security2_u1_end_deadline --test security2_u1_opening --no-fail-fast`, exit 101.

`security2_u1_end_deadline.rs` has 2 cases, both red. Read timeout 300 ms. After the end-of-input commit the server sends one message every 100 ms for 12 s and never answers the commit. Each case expects no single `next()` call to block longer than 3 s (ten read timeouts), and then a `Stalled` failure.
- `warnings_streamed_…`: the server sends only `warning`. Red:
  ```
  a single next() after the end of input was still blocked after 3s (read timeout 300ms) while the server sent only `warning` messages every 100ms and never answered the end-of-input commit
  ```
- `the_same_partial_streamed_…`: the server repeats the partial it already sent. Red:
  ```
  a single next() after the end of input was still blocked after 3s (read timeout 300ms) while the server sent only the partial transcript it had already sent every 100ms and never answered the end-of-input commit
  ```

`security2_u1_opening.rs` has 3 cases: 1 red, 2 green.
- `a_credential_lookup_that_blocks_its_thread_…`: the credential source's future sleeps on its thread for 6 s; connect timeout is 1 s. The case expects the recognition to end within 3 s with `Connect`. Red:
  ```
  the opening, the credential included, is documented as bounded by the connect timeout (1s), yet the recognition took 6.001384556s to end, failure Some(Connect(ConnectError { status: None, detail: "no connection within 1s" }))
  ```
- `a_credential_that_never_resolves_…`: the credential future never resolves. Green: the recognition fails as `Connect` within 3 s and the server sees no connection.
- `a_handshake_answered_after_the_read_timeout_past_the_end_of_input_still_delivers`: the server delays the upgrade by 900 ms, past the 300 ms read timeout after input ends, then answers the commit. Green: `["hypothesis eins", "commit eins", "usage"]`, a final record, no failure. This closes the "slow but successful handshake" half of pass-1 finding 1.

**3. Suite run (after the cases existed)**

Command: `cargo test -p inference-elevenlabs --all-features --no-fail-fast`, exit 101.
- Every binary except mine reported `test result: ok`, including `security_u1_stt` (4 passed: all pass-1 cases now green), `stt` (15), `stt_conformance` (8) and `adversary_u1_end_of_input` (3).
- `security2_u1_end_deadline` reported `FAILED. 0 passed; 2 failed`.
- `security2_u1_opening` reported `FAILED. 2 passed; 1 failed`.
- Totals: 142 passed, 3 failed, 3 ignored.
- The "before" count of 140 is this same run with my two files excluded (145 − 5). No `adversary2_u1_*` file existed when the suite ran.

**4. Findings (cover fc0b0b0d)**

| # | file:line | Verdict | Origin | What was measured | What reaches it |
|---|---|---|---|---|---|
| 1 | `src/recogniser.rs:466` | NEEDS-CHANGE | introduced | Every server text message after the end of input resets `heard_at`, and the read timeout is measured from it. The correction thereby replaced 141a2f20's one fixed deadline with a sliding one that has no cap. A server that keeps sending messages that deliver nothing (`warning`, the same partial, `session_started`, an unknown type) blocks one `next()` call for as long as it keeps sending. The caller cannot cancel from another thread, because `cancel` needs `&mut self`. A stream of changing partials also never ends, but the caller at least receives events. This contradicts "never a panic or a hang" in `lib.rs`. The Deepgram recogniser uses one deadline that messages never move (`inference-deepgram/src/recogniser.rs:615`). | Any server, broken or hostile, after the end of input. The default endpoint and the unit's own fixture both reach it. **Correction:** keep the sliding timeout that the adversary case `a_server_that_keeps_talking_…` needs, but refresh `heard_at` only on a message that delivers an event or answers a commit; or add an overall cap from the end of input (or from the opening, when later). |
| 2 | `src/recogniser.rs:443` | INFEASIBLE | introduced | Once input has ended and before `Opened`, the handle waits with `recv()` and no deadline of its own. It relies on the worker's `tokio::time::timeout(connect_timeout, …)` at `:718`. On the worker's current-thread runtime, that timeout cannot fire while a credential future blocks its thread, so the recognition ends after the lookup (6 s), not after the connect timeout (1 s). This contradicts "the opening, the credential included, is bounded by the connect timeout" (`lib.rs`, `drive` doc). | No shipped source reaches it: `EnvironmentSource` resolves at once and `KeychainSource` uses `spawn_blocking` (secrets v0.6.0, `secrets-keychain/src/lib.rs:166`). It needs a caller-written `CredentialSource` that does synchronous I/O in its future. **Correction:** use `recv_timeout(connect_timeout + slack)` in the pending branch, or say in the doc that the source's future must not block. |

**5. Checked and could not fault**

- **Pass-1 #1 (handshake pending at end of input), as a class:**
  - An unanswered handshake fails as `Connect` (pass-1 case, green).
  - A late but successful handshake delivers its transcript (my case, green).
  - `Opened` is the worker's first message, and the read timeout starts from it.
  - An unanswered commit is now `Stalled`, not a clean end.
- **Pass-1 #2 (message size), as a class:** `MAX_MESSAGE_SIZE` is set as both message and frame bound at the single `connect_async_tls_with_config` call in `connection.rs`, which TTS and STT share. tungstenite 0.30 also checks fragmented messages (`IncompleteMessage::extend`, `protocol/mod.rs:686/707`). `agent_duplex.rs:513` keeps its own 1 MiB. At most 33 × 4 MiB can be queued.
  - One open question: whether a real TTS audio message can exceed 4 MiB (about 35 s of `pcm_44100` in one message) only the TTS live test can show.
- **Pass-1 #3 (overflow), as a class:**
  - `heard_at` uses `checked_add` for both deadlines.
  - `recv_timeout` and `tokio::time::timeout` both cope with huge durations.
  - `Connection::deadline` uses `checked_add`.
  - No unchecked `Instant + Duration` remains in `recogniser.rs` or `connection.rs`.
- **Pass-1 #4 (timing), as a class:** words with `start < 0` or `end < start` are dropped. The span is a min/max over valid words, so word order cannot give `end < start`. `serde_json` rejects NaN and infinity. There is no check against the amount of audio sent, but the doc qualifies the timing as "as the endpoint timed its words".
- **Credential path:**
  - The key is resolved before any TCP connection; my never-resolving case shows the server sees none.
  - A credential future that never resolves is bounded by the connect timeout.
  - The key is still sent only in `xi-api-key`.
  - The correction added no new error text that carries the key or its source.
- **Teardown:** if the handle is gone when the connection opens, the worker closes the connection and returns. A cancel or drop during the opening still drops the attempt.
- **Display change:** `Runtime` now displays as "the runtime failed: …", with no variant or field change.

**6. Paths written outside the worktree**

- `<scratch>/wave-p1/U1/security2/` (directory created)
- `<scratch>/wave-p1/U1/security2/cases-alone.log`
- `<scratch>/wave-p1/U1/security2/suite.log`
- `<scratch>/wave-p1/U1/security2/status-before-suite.txt`

The build went to the worktree's own `target/`. I took the worktree lease as session `f7875192-U1-security2` and released it at the end.

**7. Findings block**

```findings
- file: adapters/inference-elevenlabs/src/recogniser.rs
  line: 466
  category: boundary
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: every server message after the end of input moves the read-timeout start with no cap, so a server repeating warnings or an unchanged partial while never answering the commit keeps one next() call blocked indefinitely, contradicting never a hang
- file: adapters/inference-elevenlabs/src/recogniser.rs
  line: 443
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: before the connection opens the handle waits with an unbounded recv and the worker's connect-timeout cannot fire while a caller-written credential future blocks its thread, so the documented credential-inclusive connect bound holds only for sources that yield; no shipped source blocks
```
