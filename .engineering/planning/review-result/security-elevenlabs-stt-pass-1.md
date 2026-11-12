---
format: aep.planning-md/3
id: review-result:security-elevenlabs-stt-pass-1
kind: review-result
status: active
title: Security review of the ElevenLabs realtime STT adapter, pass 1
relations:
- reviews: story:elevenlabs-realtime-stt-adapter
revision: 2
---
unit: story:elevenlabs-realtime-stt-adapter, commit 141a2f20 (base 1644e13f) plus the uncommitted test file below
verdict: NEEDS-CHANGE
cases: executed 132→136, red 4
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 4 paths (the scratch directory, two logs, one counts file)
needs-coordinator: no

**1. Diff proof**

`git --no-pager diff --stat` is empty because no tracked file changed. `git status --short` shows one new file, and it is a test file:
```
?? adapters/inference-elevenlabs/tests/security_u1_stt.rs
```

**2. Cases added**

All four are in `adapters/inference-elevenlabs/tests/security_u1_stt.rs`. The file starts with `#![cfg(feature = "stt")]` and reuses `tests/stt_support`. All four are red now. The output below is from running that file alone, before the suite (`cargo test -p inference-elevenlabs --all-features --test security_u1_stt`, exit 101):

- **Pending handshake ends as a clean success.** The server accepts TCP and never answers the upgrade; connect timeout 2 s, read timeout 300 ms. The test expects `Connect` and a non-final usage record.
  ```
  the handshake was never answered, yet the recognition ended with failure None
  ```
- **Oversized server message is delivered.** One `partial_transcript` of 4 MiB + 1 KiB, just above the 4 MiB `MAX_MESSAGE_SIZE` the Deepgram recogniser uses. The test expects no delivered text over 4 MiB and a `Transport` or `Protocol` failure.
  ```
  a 4195328-byte transcript was delivered from a single server message; failure Some(Stalled(2s))
  ```
- **Large read timeout panics.** `read_timeout(Duration::MAX)`. The test expects no panic on the caller's thread; the TTS path of this crate uses `checked_add` in `connection.rs:78`.
  ```
  thread '<unnamed>' panicked at adapters/inference-elevenlabs/src/recogniser.rs:410:31:
  overflow when adding duration to instant
  ```
- **Impossible span timing is accepted.** Words `{start 2.0, end 1.0}` and `{start -3.0, end 0.5}`. The test expects either a `Protocol` error, or a timing that starts at or after 0 and ends at or after its start.
  ```
  [{"end":1.0,"start":2.0,"text":"a","type":"word"}]: timing Some(SpanTiming { start: 2.0, end: 1.0 }) does not lie in the audio sent
  ```

**3. Suite run (after the cases existed)**

Command: `cargo test -p inference-elevenlabs --all-features --no-fail-fast`, exit 101. I added `--no-fail-fast` so every test binary runs and the count is complete. Log: `.../security/suite.log`.

- Every binary except mine reported `test result: ok`.
- `security_u1_stt` reported `test result: FAILED. 0 passed; 4 failed`.
- Totals: 132 passed, 4 failed, 3 ignored (live tests).
- The "before" count of 132 comes from this same run with `security_u1_stt` excluded.

**4. Findings (cover 141a2f20)**

| # | file:line | Verdict | Origin | What was measured | What reaches it |
|---|---|---|---|---|---|
| 1 | `src/recogniser.rs:410` | NEEDS-CHANGE | introduced | The read-timeout deadline starts when input ends, whether or not the connection is open. If the handshake is still pending when it passes, `quiet()` (`:477`) finishes cleanly. Result: no failure, a final usage record of 60 ms, and audio that never left the machine. A handshake that is slow but succeeds loses its transcript the same way. | Any `Endpoint` with connect timeout longer than read timeout; the unit's own fixture `endpoint()` uses 5 s / 2 s. Input ends before the handshake completes, as when a clip is fed in a burst like `recognise()` does. With the default 10 s / 10 s it is a race. **Correction:** start `due` from the moment the connection opens, or wait for the open/failed outcome before timing out. |
| 2 | `src/connection.rs:60` | NEEDS-CHANGE | introduced | `connect_async_tls_with_config(request, None, …)` keeps tungstenite's defaults: 64 MiB per message, 16 MiB per frame. The STT path delivers a 4 MiB transcript, and up to 33 such messages can sit in the queue (`STT_INBOUND_DEPTH` plus one held). | Any hostile or broken server on the new recogniser path. The Deepgram recogniser (4 MiB) and this crate's agent binding (1 MiB) both set a bound. The TTS path shares this call. **Correction:** pass a `WebSocketConfig` with `max_message_size` and `max_frame_size`, as `agent_duplex.rs:511` does. |
| 3 | `src/recogniser.rs:410` | INFEASIBLE | introduced | `now + read_timeout` panics on overflow, on the caller's thread. | The public `Endpoint::with_read_timeout` accepts the value. TTS treats an overflowing deadline as no deadline. I found no caller that passes `Duration::MAX`. **Correction:** `checked_add`, as `connection.rs:78`. |
| 4 | `src/stt_protocol.rs:554` | CONFIRMED | introduced | A word with `end` before `start`, or a negative `start`, becomes a `SpanTiming` that does not lie in the audio sent. | Only a server sending such timings; this is about untrusted input reaching a public type. |

**5. Checked and could not fault**

- **Where the key travels:** only in `xi-api-key` through the shared `Connection::open`, with `Credential` zeroized. It is not in `request_url` or `ElevenLabsRecogniser::fmt`, which prints the credential name only; it is not in `ElevenLabsRecognition::fmt`, and not in error text. Errors carry tungstenite's description or the server's own text, cut to 256 characters.
- **Crate doc on key copies** covers the STT path, because it goes through the same connection. That meets story:api-key-copies-in-handshakes.
- **`ws://` loopback check** refuses `@`, `localhost.evil`, non-loopback IPs and uppercase or other schemes. Rejecting a query or fragment holds.
- **Query encoding** of model, language and key terms: no parameter can be injected.
- **Bounded deadlines:** writes use the read timeout, close uses `CLOSE_BOUND`, the handshake uses the connect timeout. A cancel or drop during the handshake drops the attempt at once.
- **No hang on teardown:** the worker's blocking `inbound.send` ends when the handle drops.
- **Error types:** all fourteen documented types map one-to-one to variants. An unknown type that carries `error` becomes `Server`; a binary message becomes `Protocol`.
- **Concurrency slot** is released on refusal and on drop.
- **No obvious panic:** no indexing or unwrap on server input in `parse_server_message`, `timing`, `hear` or `commit_open`.
- **Warning text is bounded:** each kept warning is cut to 256 characters, and at most 16 are kept.

**6. Paths written outside the worktree**

- `<scratch>/wave-p1/U1/security/` (directory created)
- `<scratch>/wave-p1/U1/security/case-alone.log`
- `<scratch>/wave-p1/U1/security/suite.log`
- `<scratch>/wave-p1/U1/security/counts.txt`

The build went to the worktree's own `target/`. I took the worktree lease as session `f7875192-U1-security` and released it at the end.

**7. Findings block**

```findings
- file: adapters/inference-elevenlabs/src/recogniser.rs
  line: 410
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the end-of-input deadline runs from end of input even while the handshake is pending, so an unanswered or slow handshake ends as a clean, final recognition with no failure and the audio never sent
- file: adapters/inference-elevenlabs/src/connection.rs
  line: 60
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the STT connection keeps tungstenite's 64 MiB message limit, so a single 4 MiB server transcript is delivered where the crate's agent and the Deepgram recogniser bound messages at 1 and 4 MiB
- file: adapters/inference-elevenlabs/src/recogniser.rs
  line: 410
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: Instant plus read_timeout panics on the caller's thread for a read timeout too large for an Instant, where the TTS path uses checked_add; no caller passing such a value was found
- file: adapters/inference-elevenlabs/src/stt_protocol.rs
  line: 554
  category: integrity
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: word timings ending before they start or starting before zero are admitted into SpanTiming, which claims to say where the span lies in the audio sent
```
