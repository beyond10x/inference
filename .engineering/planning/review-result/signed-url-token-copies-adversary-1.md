---
format: aep.planning-md/3
id: review-result:signed-url-token-copies-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the signed-URL token copies
relations:
- reviews: story:signed-url-token-copies
revision: 1
---
Adversary pass 1 on story:signed-url-token-copies, 2026-11-24, as the adversary returned it. The coordinator replaced two path prefixes outside the repository with `<worktree>` and `<scratch>` (the repository is public); nothing else is changed.

unit: story:signed-url-token-copies, worktree <worktree> at head 161028818f612e38fd7392d2b646995e2466813d plus one untracked test file
verdict: CONFIRMED
cases: executed 169→172, red 2
origin: introduced 2, pre-existing 0, undecided 0
wrote-outside-worktree: 3 paths
needs-coordinator: none

Both red cases show the unit's own documentation contradicting its code. I found no token copy the crate doc omits and no field that keeps the URL past the handshake; that part is by reading only, since no test can scan memory.

## 1. What I touched

`git --no-pager diff --stat` prints nothing, because the only change is an untracked file. `git status --short`:

```
?? adapters/inference-elevenlabs/tests/adversary_w2_u3_signed_url.rs
```

No implementation file was changed, nothing is committed or staged, and my lease `w2-u3-adversary` is released.

## 2. Cases added

File: `<worktree>/adapters/inference-elevenlabs/tests/adversary_w2_u3_signed_url.rs`

| case | asserts | now |
|---|---|---|
| `a_signed_model_refused_for_its_formats_still_has_its_url_for_the_corrected_open` | An agent reports `pcm_16000`/`pcm_24000`. The first open (16 kHz both ways) is refused as `Unsupported::OutputFormat` and `open_error()` names the formats. The second open with those formats succeeds, as `agent_duplex.rs:33` promises. | red |
| `a_signed_model_whose_open_met_a_closed_port_still_has_its_url_for_one_that_connects` | The first open meets a closed port (`Connect { status: None }`). A server then listens on that port and the second open succeeds, as "the first open that connects" at `agent_duplex.rs:400` says. | red |
| `a_public_model_refused_for_its_formats_opens_with_the_formats_the_error_named` | Control: the same format flow on a public model opens, so the fixture and the corrected configuration are sound. | green |

Each case runs in about 10 ms; none is over 5 s.

Red run of the file alone, before any suite run (`CARGO_INCREMENTAL=0 cargo nextest run -p inference-elevenlabs --all-features --test adversary_w2_u3_signed_url`):

```
    Starting 3 tests across 1 binary
        FAIL [   0.009s] (1/3) inference-elevenlabs::adversary_w2_u3_signed_url a_signed_model_refused_for_its_formats_still_has_its_url_for_the_corrected_open
    thread 'a_signed_model_refused_for_its_formats_still_has_its_url_for_the_corrected_open' (299668) panicked at adapters/inference-elevenlabs/tests/adversary_w2_u3_signed_url.rs:123:5:
    an open refused for its configuration did not leave the URL with the model: Some(Failed(SessionFailure { kind: Rejected, detail: "the signed URL went to an earlier open of this model and opens one session; build a model with a new one" })) / Some(SignedUrlSpent)
        FAIL [   0.010s] (2/3) inference-elevenlabs::adversary_w2_u3_signed_url a_signed_model_whose_open_met_a_closed_port_still_has_its_url_for_one_that_connects
    thread 'a_signed_model_whose_open_met_a_closed_port_still_has_its_url_for_one_that_connects' (299672) panicked at adapters/inference-elevenlabs/tests/adversary_w2_u3_signed_url.rs:162:5:
    no open connected before this one, and the model has no URL for it: Some(Failed(SessionFailure { kind: Rejected, detail: "the signed URL went to an earlier open of this model and opens one session; build a model with a new one" })) / Some(SignedUrlSpent)
        PASS [   0.012s] (3/3) inference-elevenlabs::adversary_w2_u3_signed_url a_public_model_refused_for_its_formats_opens_with_the_formats_the_error_named
     Summary [   0.012s] 3 tests run: 1 passed, 2 failed, 0 skipped
error: test run failed
exit 100
```

After this run I joined two lines in the control test to satisfy rustfmt. No assertion or line number changed, and the suite run below is of the final file. With the file in place, `cargo fmt --all --check` exits 0 and `cargo clippy -p inference-elevenlabs --all-targets --all-features -- -D warnings` exits 0.

## 3. Suite run, after the cases existed

`CARGO_INCREMENTAL=0 cargo nextest run -p inference-elevenlabs --all-features`

```
    Starting 172 tests across 33 binaries (5 tests skipped)
     Summary [   5.459s] 172 tests run: 168 passed, 4 failed, 5 skipped
        FAIL [   0.006s] ( 50/172) inference-elevenlabs::adversary_w2_u3_signed_url a_signed_model_whose_open_met_a_closed_port_still_has_its_url_for_one_that_connects
        FAIL [   0.008s] ( 51/172) inference-elevenlabs::adversary_w2_u3_signed_url a_signed_model_refused_for_its_formats_still_has_its_url_for_the_corrected_open
        FAIL [   0.372s] ( 95/172) inference-elevenlabs::async_handles twenty_recognitions_wait_for_the_server_without_blocking_the_runtime
        FAIL [   0.612s] (114/172) inference-elevenlabs::async_handles twenty_syntheses_wait_for_the_server_without_blocking_the_runtime
error: test run failed
exit 100
```

Two of the four failures are mine; the other two are the known ticker cases the brief excludes. The `before` count of 169 is the implementor's `green.log` summary ("169 tests run: 167 passed, 2 failed, 5 skipped"), not a run of mine.

## 4. Findings

They cover head 161028818 plus the untracked file.

| # | file:line | verdict | origin | finding |
|---|---|---|---|---|
| 1 | `adapters/inference-elevenlabs/src/agent_duplex.rs:33` | CONFIRMED (warning) | introduced | The module doc says "An open refused for its configuration leaves the URL with the model." An open the agent refuses for its formats (`OpenError::Unsupported`, `AgentError::FormatMismatch`) has already run the handshake and spent the URL, so the corrected open fails as `SignedUrlSpent`. |
| 2 | `adapters/inference-elevenlabs/src/agent_duplex.rs:400` | CONFIRMED (note) | introduced | The doc of `ElevenLabsAgent::signed` says "the first open that connects gives `signed` to its handshake". An open that met a closed port connected to nothing, sent the token nowhere, and still spent the URL. |

**Finding 1**
- **Measured:** the assertion at `tests/adversary_w2_u3_signed_url.rs:123`, exit 100; the control on a public model is green.
- **What reaches it:** any caller of `ElevenLabsAgent::signed` whose session formats differ from the agent's and who corrects them from `open_error()`. That is the workflow the same module doc describes at lines 18-21 ("`open_error` naming what it reported").
- **Why it matters:** with a signed URL, learning the agent's formats now costs one URL, and the doc says the opposite. `lib.rs:89-90` and `agent_duplex.rs:26-27` ("first open whose configuration the binding takes") do not say it either.
- **Fix, not applied:** say "refused before it connects" at line 33, and state that a format refusal spends the URL.

**Finding 2**
- **Measured:** the assertion at `tests/adversary_w2_u3_signed_url.rs:162`, exit 100.
- **What reaches it:** a caller that retries the same model after a failed connect.
- **Context:** the module doc and `lib.rs` state the real behaviour ("refused, timed out or was cancelled"), so only the constructor's sentence is wrong.
- **Fix, not applied:** "the first open that reaches the handshake, connected or not".

**Origin basis:** read, not run on the base. Both sentences and `SignedUrlSpent` are added by this diff, and the base's `open` passes `&self.target` to `connect` on every open (`git show 9f8f013e:adapters/inference-elevenlabs/src/agent_duplex.rs`, lines 431-436).

## 5. Attacked and not broken

- **Unlisted copies in the client libraries:** none found. I read tungstenite 0.30.0 `handshake/client.rs` (`generate_request`), tokio-tungstenite 0.30.0 `connect.rs` and http 1.5.0 `uri/mod.rs:281-290`. The request is moved uncloned, the URI is the caller's buffer, and the only copy is the request bytes the doc lists.
- **Logs:** the only log line carrying the request is the trace at `handshake/client.rs:197`. The redirect, "Trying to contact {uri}" and `UnableToConnect(uri)` lines (`client.rs:98,132,137`) belong to the sync client, which tokio-tungstenite does not call.
- **Error text and `Debug`:** no tungstenite error on this path prints the URI (`Error::Http` prints the status only), and `Dial`, `Target` and `UrlBytes` have no `Debug`.
- **Zeroizing the bytes the `Uri` holds:** it is the same buffer, so it cannot be zeroized while the request lives, and is when the last `Arc` drops.
- **Cancel, timeout, dropped future:** `open` blocks on the calling thread and the `handshake` future owns the `Dial`; both `select!` arms drop it. No test pins this: holders are counted only after a connected and a connection-refused handshake.
- **Two opens racing:** impossible, since `open` takes `&mut self` and nothing else reaches `Target::dial`.
- **`Weak` count 0 while a byte copy exists:** the count sees `Arc` holders only. Absence of a byte copy rests on the pointer test of `signed_request` and on reading; `unsafe_code = "forbid"` (`Cargo.toml:21`) rules out a memory scan from `tests/` too.
- **The `compare` claim:** `measure_arm` (`examples/realtime-voice/src/compare.rs:1596-1626`) returns the first failure and records later ones as notes, as documented.
- **Generated docs:** `website/docs/reference/errors.md` does not list `AgentError`. I did not run the generator check, since the brief limits builds to `-p inference-elevenlabs`.
- **Live test:** `tests/agent_duplex_live.rs` builds a new model per test, so the one-session rule does not break it.

## 6. Paths written outside the worktree

- `<scratch>/adversary-1/red-alone.log`
- `<scratch>/adversary-1/suite.log`
- `<scratch>/adversary-1/clippy.log`

## 7. Findings block

```findings
- file: adapters/inference-elevenlabs/src/agent_duplex.rs
  line: 33
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the module doc says an open refused for its configuration leaves the URL with the model, but an open the agent refuses for its formats has spent it and the corrected open fails as SignedUrlSpent
- file: adapters/inference-elevenlabs/src/agent_duplex.rs
  line: 400
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the doc of ElevenLabsAgent::signed says the first open that connects takes the URL, but an open that met a closed port and connected to nothing spends it too
```
