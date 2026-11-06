---
format: aep.planning-md/3
id: review-result:security-deepgram-stt-pass-1
kind: review-result
status: active
title: Security review of the Deepgram streaming STT adapter, pass 1
relations:
- reviews: story:deepgram-streaming-stt-adapter
revision: 1
---
unit: U2 story:deepgram-streaming-stt-adapter, the tree `inference-0-17-0-u2-sec` at `98bff37`, plus my test-only commit `d825c97` (`b10x-bot[bot]` as author and committer)
verdict: CONFIRMED
cases: executed 50→58, red 2
origin: introduced 4 / pre-existing 2 / undecided 0
wrote-outside-worktree: 6 paths in the U2 scratch directory
needs-coordinator: none

**What I changed** (`git diff --stat 98bff37 HEAD`): only two new test files. `adapters/inference-deepgram/tests/security1_boundaries.rs` has 327 lines and `security1_release.rs` has 79, 406 lines in total. No implementation file was touched.

**Each case run alone first**, with `--all-features --test <file>`:
- `security1_boundaries` gave 5 passed and 2 failed. The two failures:
  - `an_empty_or_blank_key_is_refused_before_any_connection`: `key "": expected a typed credential refusal, got None`
  - `a_cancel_during_the_opening_handshake_closes_the_connection_within_the_close_bound`: `the connection stayed open 8.019466135s after the cancel, while the slot was already free`
- `security1_release` gave 1 passed.

**Full suite run**: `cargo-run.sh test -p inference-deepgram --tests --all-features --no-fail-fast`, exit code 101.
- lib 14, audio 7, auth 5, cancel 2, claims_record 1, conformance 8, errors 8, transcripts 5, `live` 0 (1 ignored).
- `security1_boundaries`: 5 passed, 2 failed. `security1_release`: 1 passed.
- clippy with `--all-targets --all-features -D warnings` and rustfmt both pass. `scan-text` found no private rule.

**Each invariant from the brief, held or not:**
- **Key never in Debug, Display, an error, the URL or the query: held.** 11 failure paths built with a recognisable key; none renders the key or part of it. `request_url` for the default and EU hosts carries no key.
- **Key from a library dependency in a log: not held, and documented.** tungstenite logs the whole handshake request, `Authorization` header included, at trace level (`tungstenite-0.30.0/src/handshake/client.rs:197`). The crate doc says so.
- **Key memory: partly held.** `Credential` and the header value the binding builds are zeroized, and the key is fetched again for each connection. The key is also copied into `HeaderValue` and into tungstenite's request buffer, and neither is zeroized.
- **Credential sources: partly held.** A key containing CR, LF or NUL gives `Credential(Invalid)` before any connection. An empty or blank key is not refused: the binding sends `Token ` and connects.
- **Transport: held.** All 16 look-alike URLs are refused, and the error message names no URL. TLS uses webpki roots with the ring provider; no path accepts invalid certificates (`connection.rs:262`). The EU host is a constant.
- **Untrusted server input: held for panics.** Deep JSON, `1e400`, `{}`, non-numeric `last_word_end`, a 1 MB transcript and out-of-range numbers gave a typed `Protocol` error or completed. Memory while the caller does not iterate, and total time after `CloseStream`, are not bounded.
- **Transcript text never logged: held.** No `log` or `tracing` dependency.
- **Resources released: held, with one gap.** 24 recognitions leave no driving thread after 3 s. A cancel or drop during the opening handshake keeps the connection open until the connect timeout.

Cargo time 1.4 minutes, wall about 30 minutes, peak `target/` 1.2G.

```findings
[
{"file":"adapters/inference-deepgram/src/connection.rs","line":220,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"An empty or blank key is sent as `Authorization: Token ` and the handshake proceeds instead of a typed Credential refusal before connecting (security1_boundaries::an_empty_or_blank_key_is_refused_before_any_connection, red)."},
{"file":"adapters/inference-deepgram/src/recogniser.rs","line":544,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"A cancel or drop during the opening handshake is not read until connect_timeout expires, so the socket stays open about 8 s past CLOSE_BOUND while the concurrency slot is already free (security1_boundaries::a_cancel_during_the_opening_handshake_closes_the_connection_within_the_close_bound, red)."},
{"file":"adapters/inference-deepgram/src/recogniser.rs","line":518,"category":"integrity","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"Server messages queue in an unbounded std mpsc channel with no backpressure while the caller does not iterate, each message capped only by tungstenite's default 64 MiB since no WebSocketConfig is passed at connection.rs:241."},
{"file":"adapters/inference-deepgram/src/recogniser.rs","line":583,"category":"judgement","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"After CloseStream any text message, including unknown types, resets the read deadline, so a server can hold a recognition open indefinitely though each single read is bounded."},
{"file":"adapters/inference-deepgram/src/connection.rs","line":224,"category":"judgement","severity":"note","verdict":"NEEDS-CHANGE","origin":"pre-existing","message":"The key is copied into a non-zeroizing HeaderValue and into tungstenite's request buffer, as inference-elevenlabs/src/connection.rs:46 already does."},
{"file":"adapters/inference-deepgram/src/lib.rs","line":60,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"pre-existing","message":"tungstenite logs the raw handshake request including the Authorization header at trace level (tungstenite-0.30.0/src/handshake/client.rs:197); documented as in inference-elevenlabs/src/lib.rs:41, and untestable here without a `log` dev-dependency."}
]
```
