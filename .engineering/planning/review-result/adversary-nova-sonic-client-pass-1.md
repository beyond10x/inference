---
format: aep.planning-md/3
id: review-result:adversary-nova-sonic-client-pass-1
kind: review-result
status: active
title: Adversary, story:nova-sonic-client, pass 1
relations:
- reviews: story:nova-sonic-client
revision: 1
---
```
unit: story:nova-sonic-client, uncommitted working tree of impl/nova-sonic-client on base cdf2586
verdict: NEEDS-CHANGE
cases: executed 35→45, red 1
origin: introduced 6 / pre-existing 0 / undecided 0
wrote-outside-worktree: 20 paths, all under scratch/adversary-1/ (list in part 6)
needs-coordinator: none
```

**1. Diff.** `git --no-pager diff --stat` shows only the implementor's two tracked files, `Cargo.lock` (1624 lines) and `Cargo.toml` (2 lines). I did not touch either. The whole of `adapters/` is untracked. Inside it, my additions are three test files and no implementation file:
- `adapters/inference-aws/tests/adversary1_redaction.rs`
- `adapters/inference-aws/tests/adversary1_session.rs`
- `adapters/inference-aws/tests/adversary1_decode.rs`

**2. Cases added** (each file run alone before the suite)

| File | Case | Now |
|---|---|---|
| adversary1_redaction.rs | `a_connect_error_names_no_profile_or_sso_session` | **red** |
| adversary1_session.rs | the caller's voice, output rate, tools, turn detection and inference values reach the opening events | green (guard) |
| adversary1_session.rs | the reader still works after a stall | green |
| adversary1_session.rs | all 5 roles and `interactive: true` reach the wire | green (guard) |
| adversary1_session.rs | `HIGH`, `MEDIUM` and `LOW` reach the wire | green (guard) |
| adversary1_decode.rs | ending one content block keeps an interleaved block's role and stage | green (guard) |
| adversary1_decode.rs | a stage given as an object; tool input given as an object | green (guard) |
| adversary1_decode.rs | a 1 MiB audio chunk; `{"event":{}}` gives an error | green |

Red output of the redaction case, run alone (exit 101), verbatim:
```
the connect error names the operator's profile or sso-session: dispatch failure: other: the credentials provider was not properly configured: ProfileFile provider could not be built: sso-session named `adv1-private-sso-session` (referenced by profile `adv1-private-profile`) was not found (DispatchFailure(...))
test result: FAILED. 0 passed; 1 failed
```
The case makes no network call. The credential provider fails while reading a profile file held in memory, and the endpoint is the closed local port `127.0.0.1:9`.

**Mutants.** I ran 11, in a source copy in scratch; the copy and its `target/` are deleted. Each one left all 35 of the unit's cases green, and each turned at least one of my guard cases red:
- **M1–M5** (`session.rs:43-52`): the session ignores the caller's voice, output rate, tools, turn detection or inference values and sends the defaults.
- **M6:** `interactive` is always `false`.
- **M7:** `ASSISTANT` is sent as `USER`.
- **M8:** `HIGH` is sent as `LOW`.
- **M9:** a stage given as an object is dropped.
- **M10:** tool input given as an object is rejected.
- **M11:** a content end clears every tracked block, not just its own.

**3. Gate** (after my cases existed)
- `cargo test -p inference-aws --all-features --no-fail-fast`: exit 101. Unit cases: decode 12 ok, events 10 ok, session 13 ok, live 0 run / 1 ignored. My cases: 9 ok, 1 FAILED (the redaction case).
- `cargo clippy -p inference-aws --all-targets --all-features -- -D warnings`: exit 0.
- `cargo fmt -p inference-aws --check`: exit 0.

**4. Findings** (they cover the working tree on cdf2586; the crate is new, so every finding is `introduced`)

| # | file:line | Verdict | What was measured | What reaches it |
|---|---|---|---|---|
| 1 | src/nova_sonic/client.rs:82 | NEEDS-CHANGE | Connect errors are only scrubbed of ARNs and 12-digit numbers. Profile and sso-session names come through, along with the full Debug chain of the error. The same applies to stream errors at :133. | The live test prints this error when the stream does not open (tests/nova_sonic_live.rs:73). That covers `from_env` with `AWS_PROFILE` pointing at a misconfigured profile; aws-config stops the chain on that error (aws-config credentials.rs:186-193). From reading code, not run: an SSO token cache miss also prints its file path, which includes the home directory (aws-config sso/cache.rs:90). Suggested fix: build the message from the error kind and service error code only, not from `DisplayErrorContext`. |
| 2 | src/nova_sonic/session.rs:43 | CONFIRMED | 11 mutants survive the unit's 35 cases. The guard cases above now catch them. | Any caller that sets a non-default voice, output rate, tools or turn detection. |
| 3 | tests/nova_sonic_live.rs:62 | CONFIRMED (note) | The model id is printed as given. An application inference profile ARN carries an account id. | Only if the operator sets `INFERENCE_NOVA_SONIC_MODEL` to an ARN. Nothing found that does. |
| 4 | src/nova_sonic/output.rs:244 | CONFIRMED (note) | The barge-in notice `{ "interrupted" : true }` comes out as ordinary assistant text; it is not typed. That is the string the reference's own sample checks for. The live test adds it to the assistant text (live.rs:103), so its "an assistant text" assertion could pass on the notice alone. | Inferred from code, not run. The story does not require a typed barge-in. |
| 5 | src/nova_sonic/client.rs:85 | CONFIRMED (note) | When the reader is dropped, the bridge task keeps waiting on the service; nothing tells it the reader is gone. It exits only on the service's next frame or when the service closes the stream. | Inferred; this cannot be run offline. |
| 6 | src/nova_sonic/session.rs:210 | CONFIRMED (note) | The read timeout also runs while the user is still speaking, so a user turn longer than 30 s reports `Stall`. The reader recovers afterwards (measured, green). | Inferred for real sessions. |

**5. Attacked and could not break**
- **Event encodings:** all 9 input events match `sonic-input-events` field by field, including base64 audio, the pairing of prompt and content names, and the order (session start, prompt start, system prompt, audio content start, the closing events).
- **Decoder:** interleaved blocks, text with no preceding content start, unknown events, malformed JSON, a 1 MiB audio chunk.
- **Session:** sending after close cannot compile, because `close` consumes the writer. A tool result for an unknown tool use id is passed through; that is a design choice, not a defect.
- **Live test:** its asserts are not vacuous. It needs a USER-role transcript, and a completion end that arrives before the stream ends.
- **Dependencies:** 152 crates added, including `aws-lc-sys`. They are all needed by the SDK and the default credential chain, and they include `h2`, offered through ALPN. Whether the service actually negotiates HTTP/2 is still unverified until the live run.
- **Leaks:** nothing private appears in the committed crate files.

**6. Paths written outside the worktree** (all under `<unit scratch>/adversary-1/`):
- `mutants.sh`, `mutants.log`
- `mutant-build.log`, `mutant-M1.log` … `mutant-M11.log`
- `red-redaction.log`, `new-cases.log`, `suite.log`, `clippy.log`, `fmt.log`

The `mutant-copy/` directory and its `target/` were deleted; scratch now totals 128K. The lease is released.

**7. Findings block**
```findings
- file: adapters/inference-aws/src/nova_sonic/client.rs
  line: 82
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: Connect and stream error messages carry the operator's profile and sso-session names (and, by code reading, the SSO cache path), and the live test prints them on a failed start
- file: adapters/inference-aws/src/nova_sonic/session.rs
  line: 43
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: eleven mutants (config passthrough, interactive flag, role and endpointing wire names, decoder object fields, content-end tracking) survived the unit's 35 cases; adversary1 guards now catch each
- file: adapters/inference-aws/tests/nova_sonic_live.rs
  line: 62
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the live test prints the model id unredacted, which leaks an account id when the override is an inference-profile ARN
- file: adapters/inference-aws/src/nova_sonic/output.rs
  line: 244
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: 'the barge-in notice {"interrupted": true} surfaces as plain assistant text, so consumers speak or log it and the live assistant-text assertion can be met by it'
- file: adapters/inference-aws/src/nova_sonic/client.rs
  line: 85
  category: concurrency
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the bridge task does not watch for the reader being dropped and stays parked on a silent service
- file: adapters/inference-aws/src/nova_sonic/session.rs
  line: 210
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the read timeout counts while the user is still speaking, so a turn longer than the timeout reports Stall
```