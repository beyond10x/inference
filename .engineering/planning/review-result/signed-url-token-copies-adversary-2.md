---
format: aep.planning-md/3
id: review-result:signed-url-token-copies-adversary-2
kind: review-result
status: active
title: Adversary pass 2 on the signed-URL token copies
relations:
- reviews: story:signed-url-token-copies
revision: 1
---
Adversary pass 2, the last, on story:signed-url-token-copies, 2026-11-24, as the adversary returned it; `<scratch>` is the pass's scratch directory outside the repository.

```
unit: story:signed-url-token-copies
verdict: red
cases: executed 173→184, red 3
origin: introduced 3, pre-existing 0, undecided 0
wrote-outside-worktree: 10 paths, all under <scratch>/ (part 6)
needs-coordinator: yes
```

Three cases are red, and all three are wording of the rule against the code; none shows a token copy, a kept field, or an open that dials and keeps the URL. The acceptance statement held under a new attack: a scan of the test process's own memory finds the whole signed URL once while the model holds it and zero times after each of the six outcomes, timeout and cancel included. The coordinator's part: decide whether "dials" means a connection attempt or reaching the handshake step, and whether the two uncommitted files stay.

Findings cover head 1fd3c5a4b3fbbcee5dfd37ec87a8e0c8b1c27cb4 plus the two untracked files. Lease `w2-u3-adversary-2` is released.

## 1. What I touched

`git --no-pager diff --stat` prints nothing: the only changes are untracked files. `git status --short`:

```
?? adapters/inference-elevenlabs/tests/adversary2_w2_u3_signed_url_memory.rs
?? adapters/inference-elevenlabs/tests/adversary2_w2_u3_signed_url_rule.rs
```

No implementation file and no existing test was changed; nothing is committed or staged.

## 2. Cases added

**`adapters/inference-elevenlabs/tests/adversary2_w2_u3_signed_url_rule.rs`** (rule against code)

| case | asserts | now |
|---|---|---|
| `a_signed_model_whose_open_made_no_request_and_dialled_nothing_still_has_its_url` | `SignedUrl::new` takes a URL with `<` in its query. The first open fails as `Connect { status: None }` and the server saw 0 connections. By the rule the model still has the URL, so the second open fails the same way. | red |
| `a_signed_model_whose_open_was_cancelled_before_it_dialled_still_has_its_url` | A thread calls `cancel()` in a loop. An open that ended `Cancelled` with 0 connections dialled nothing, so by the rule the next open is not `SignedUrlSpent`. If no attempt of 40 is cancelled before it dials, nothing is asserted. | red |
| `every_later_open_of_a_spent_signed_model_fails_as_signed_url_spent` | After an open that connected, an open with `auto_response: false` leaves `open_error() == Some(SignedUrlSpent)`, as "Every later open of that model fails as `AgentError::SignedUrlSpent`" says. | red |

**`adapters/inference-elevenlabs/tests/adversary2_w2_u3_signed_url_memory.rs`** (acceptance statement, Linux only)

Each case builds the URL in one buffer, keeps only a masked copy, and counts the whole URL (scheme, host, path, query with token) in the process's writable memory through `/proc/self/mem`. It needs no `unsafe`, so the workspace's `unsafe_code = "forbid"` allows it.

| case | asserts | now |
|---|---|---|
| `the_url_of_a_model_that_has_not_dialled_stands_in_memory_once_until_the_model_drops` | Control: count 1 as built, 1 in the model, 1 after an open refused for its configuration, 0 after the model drops. | green |
| `the_url_stands_in_memory_once_while_its_handshake_waits_and_not_after` | Count 1 during a stalled handshake (scanned from a second thread), 0 after the cancel. | green |
| `no_memory_holds_the_url_while_the_session_it_opened_lives` | Count 0 with the session alive. | green |
| `no_memory_holds_the_url_after_a_connection_that_could_not_be_made` | Count 0. | green |
| `no_memory_holds_the_url_after_a_refused_upgrade` | Count 0 after HTTP 403. | green |
| `no_memory_holds_the_url_after_a_handshake_that_timed_out` | Count 0; the server saw 1 connection. | green |
| `no_memory_holds_the_url_after_a_handshake_that_was_cancelled` | Count 0; the server saw 1 connection. | green |
| `no_memory_holds_the_url_after_an_open_the_agent_refused_for_its_formats` | Count 0. | green |

The scan searches the whole URL, not the token alone. The request line and the fixture server's recorded query hold the token without scheme and host; the crate doc lists the first as an accepted copy. No case runs over 0.6 s.

First run of the two files alone, before any suite run (`CARGO_INCREMENTAL=0 cargo nextest run -p inference-elevenlabs --all-features --no-fail-fast --test adversary2_w2_u3_signed_url_rule --test adversary2_w2_u3_signed_url_memory`, `<scratch>/alone-1.log`):

```
    Starting 10 tests across 2 binaries
        FAIL [   0.010s] ( 1/10) inference-elevenlabs::adversary2_w2_u3_signed_url_rule every_later_open_of_a_spent_signed_model_fails_as_signed_url_spent
    thread 'every_later_open_of_a_spent_signed_model_fails_as_signed_url_spent' (888170) panicked at adapters/inference-elevenlabs/tests/adversary2_w2_u3_signed_url_rule.rs:159:5:
    assertion `left == right` failed: a later open of a spent model failed as something else: Some(Unsupported(ManualResponse))
      left: None
     right: Some(SignedUrlSpent)
        FAIL [   0.058s] ( 2/10) inference-elevenlabs::adversary2_w2_u3_signed_url_rule a_signed_model_whose_open_was_cancelled_before_it_dialled_still_has_its_url
    thread 'a_signed_model_whose_open_was_cancelled_before_it_dialled_still_has_its_url' (888169) panicked at adapters/inference-elevenlabs/tests/adversary2_w2_u3_signed_url_rule.rs:131:9:
    assertion `left != right` failed: attempt 1: the open was cancelled with nothing dialled, and the model has no URL for the next
      left: Some(SignedUrlSpent)
     right: Some(SignedUrlSpent)
        FAIL [   0.058s] ( 3/10) inference-elevenlabs::adversary2_w2_u3_signed_url_rule a_signed_model_whose_open_made_no_request_and_dialled_nothing_still_has_its_url
    thread 'a_signed_model_whose_open_made_no_request_and_dialled_nothing_still_has_its_url' (888171) panicked at adapters/inference-elevenlabs/tests/adversary2_w2_u3_signed_url_rule.rs:89:5:
    assertion `left == right` failed: the first open dialled nothing, and the model has no URL for the second
      left: Some(SignedUrlSpent)
     right: Some(Connect { status: None, detail: "HTTP format error: invalid uri character" })
        PASS [   0.128s] ( 4/10) ... no_memory_holds_the_url_after_a_connection_that_could_not_be_made
        PASS [   0.200s] ( 5/10) ... no_memory_holds_the_url_after_an_open_the_agent_refused_for_its_formats
        PASS [   0.232s] ( 6/10) ... no_memory_holds_the_url_after_a_refused_upgrade
        PASS [   0.383s] ( 7/10) ... no_memory_holds_the_url_after_a_handshake_that_timed_out
        PASS [   0.453s] ( 8/10) ... no_memory_holds_the_url_after_a_handshake_that_was_cancelled
        PASS [   0.469s] ( 9/10) ... no_memory_holds_the_url_while_the_session_it_opened_lives
        PASS [   0.490s] (10/10) ... the_url_of_a_model_that_has_not_dialled_stands_in_memory_once_until_the_model_drops
     Summary [   0.491s] 10 tests run: 7 passed, 3 failed, 0 skipped
exit 100
```

- **Repeats:** the same 3 red and 7 green in three more runs of the two files (`<scratch>/alone-2.log` to `alone-4.log`); the cancel case won its race at attempt 1 in all five runs.
- **Added after the first run:** `the_url_stands_in_memory_once_while_its_handshake_waits_and_not_after`, green in `<scratch>/memory-5.log` (8 of 8) and in the final suite run.
- **Checks with the files in place:** `cargo fmt --all --check` exit 0; `cargo clippy -p inference-elevenlabs --all-targets --all-features -- -D warnings` exit 0.

## 3. Suite run, after the cases existed

`CARGO_INCREMENTAL=0 cargo nextest run -p inference-elevenlabs --all-features --no-fail-fast` (`<scratch>/suite-final.log`, the final files):

```
    Starting 184 tests across 35 binaries (5 tests skipped)
     Summary [   5.644s] 184 tests run: 179 passed, 5 failed, 5 skipped
        FAIL [   0.058s] ( 44/184) inference-elevenlabs::adversary2_w2_u3_signed_url_rule a_signed_model_whose_open_made_no_request_and_dialled_nothing_still_has_its_url
        FAIL [   0.056s] ( 45/184) inference-elevenlabs::adversary2_w2_u3_signed_url_rule a_signed_model_whose_open_was_cancelled_before_it_dialled_still_has_its_url
        FAIL [   0.011s] ( 46/184) inference-elevenlabs::adversary2_w2_u3_signed_url_rule every_later_open_of_a_spent_signed_model_fails_as_signed_url_spent
        FAIL [   0.310s] (111/184) inference-elevenlabs::async_handles twenty_recognitions_wait_for_the_server_without_blocking_the_runtime
        FAIL [   0.617s] (139/184) inference-elevenlabs::async_handles twenty_syntheses_wait_for_the_server_without_blocking_the_runtime
error: test run failed
exit 100
```

Three failures are mine; two are the known `async_handles` cases the brief excludes. `before` is 173, from the implementor's `gate2-nextest.log` ("173 tests run: 171 passed, 2 failed, 5 skipped"), not a run of mine. An earlier suite run with 10 of my cases (`<scratch>/suite.log`) shows 183 run and the same 5 failures.

Generator check, which pass 1 did not run: `cargo run -q -p inference-docs -- generate --check` exits 0 (`<scratch>/docs-check.log`). `website/docs/reference/environment.md` is not among the files it reports, so its signed-URL sentence is hand-written.

## 4. Findings

| # | file:line | verdict | origin | finding |
|---|---|---|---|---|
| 1 | `adapters/inference-elevenlabs/src/agent_duplex.rs:512` | CONFIRMED (note) | introduced | The URL leaves the model at `Target::dial`, before the request is built and before the handshake is first polled. Two opens that dial nothing spend it: one whose request cannot be made from the URL, and one cancelled before the first poll. |
| 2 | `adapters/inference-elevenlabs/src/agent_duplex.rs:34` | CONFIRMED (note) | introduced | "Every later open of that model fails as `AgentError::SignedUrlSpent`" is not true of a later open whose configuration the binding refuses: it fails as `OpenError::Unsupported` and `open_error()` is `None`. |
| 3 | `adapters/inference-elevenlabs/tests/agent_duplex.rs:661` | CONFIRMED (warning) | introduced | The new timeout-and-cancel case observes the next open's refusal and the connection count, not the release of the buffer. No committed test pins "zeroized when the handshake ends" for those two outcomes. |

**Finding 1**
- **Measured:** the assertions at `tests/adversary2_w2_u3_signed_url_rule.rs:89` and `:131`, exit 100; the server counted 0 connections in both.
- **What reaches it, cancel:** `ElevenLabsAgent::canceller()` used from another thread as the open begins. The select at `agent_duplex.rs:625-628` is biased and polls the cancel arm first. My case calls `cancel()` in a loop; a single call lands in that window rarely.
- **What reaches it, request:** a signed URL with `<`, `>` or `"` in its query, or longer than 65,534 bytes; `SignedUrl::new` takes it and http 1.5.0's `Uri` refuses it. Nothing found that produces one.
- **Size:** it contradicts the rule only in the literal sense of "dials". The documents define "dials" as "starts the handshake" and list "cancelled" and "the connection could not be made" as spending, so a caller meets nothing they were not told.
- **Fix, not applied:** say "the first open that gets past its configuration and its runtime" instead of "that dials". Having `SignedUrl::new` refuse what `Uri` refuses would also close the request branch.

**Finding 2**
- **Measured:** the assertion at `tests/adversary2_w2_u3_signed_url_rule.rs:159`, exit 100.
- **What reaches it:** any caller that reopens a spent model with a configuration `SessionLedger::open` refuses. Nothing found that depends on which refusal comes first.
- **Same sentence elsewhere:** `agent_duplex.rs:407` ("Each later open fails as") and `lib.rs:96`.
- **Context:** `open_error()` being `None` after a configuration refusal is the base's behaviour too, against the base's own sentence at line 24 ("Every way an open fails leaves the reason in"). I did not raise that as a row; it is outside this unit.
- **Fix, not applied:** "every later open that gets as far as the handshake".

**Finding 3**
- **Measured, by reading:** `tests/agent_duplex.rs:661-706` asserts `SignedUrlSpent` and `connections() == 1`. The holder counts in `src/agent_duplex.rs` cover a connected and a connection-refused handshake only.
- **What reaches it:** a later change that keeps a clone of the `Arc` alive on the cancel arm (`agent_duplex.rs:627`) or the timeout arm (`:669`) passes every committed test.
- **Not run against a mutant:** a mutated copy needs a second build of the workspace, which the brief does not allow. The memory control shows the scan sees a held buffer (count 1) and its release (count 0).
- **Fix, not applied:** commit the memory file, or add two holder-count unit tests beside the existing two.

**Origin basis:** read, not run on the base. `SignedUrlSpent`, both sentences and the case at `:661` are added by this diff, and the base's `open` passes `&self.target` on every open. My cases do not compile against the base.

## 5. Attacked and not broken

- **An open that dials and keeps the URL:** none. `Target::dial` (`:512`) takes the URL before anything that can connect.
- **A refusal before the dial that loses the URL:** none. The configuration refusal (`:502`) and the runtime failure (`:510`) return before `:512`. The first is measured (memory control, count stays 1); the second is by reading. A thread-spawn failure inside tokio's `build()` would panic, also before `:512`.
- **A panic between taking the URL and the dial:** lines 512-523 are an integer add, a mutex write, `runtime.enter()` and one `Arc` allocation.
- **The buffer after each outcome:** the memory cases — the whole URL stands once while held and during a waiting handshake, and nowhere after connected, connect failed, HTTP 403, timed out, cancelled and format-refused.
- **"Takes a `String` as it is, without copying it" (`agent.rs:233`):** count is 1 after `SignedUrl::new` and 1 in the model.
- **Every other signed-URL sentence** in `adapters/inference-elevenlabs`, `website/docs` and `examples/realtime-voice` agrees with the rule, apart from findings 1 and 2. That covers `agent.rs:44-48` and `:527-530`, the `Display` of `SignedUrlSpent`, `lib.rs:87-112`, `compare.rs:25-27` and `:661`, and `environment.md:37`. `compare` opens one session per run (`examples/realtime-voice/src/compare.rs:599`).
- **Pass 1's rewritten cases, first open:** the original text is in neither git nor scratch, so I compared against pass 1's report and red log. The first-open assertions it describes (`Unsupported::OutputFormat`, `FormatMismatch`, `Connect { status: None }`) are all present, and the rewrite added exact format values.
- **Pass 1's rewritten cases, second open:** neither passes if the second open connects. Case 1 would get `Ok` (the public-model control shows the corrected configuration opens) and a second connection. Case 2 would get a `Connect` timeout instead of `SignedUrlSpent`, and the listener's `accept` would succeed.
- **The new timeout-and-cancel case, the URL given up:** it does observe that. If the model kept the URL, the second open would dial the stalling server and fail as `Connect`, and the server would count 2.

## 6. Paths written outside the worktree

All under `<scratch>/` (the `adversary-2` directory the brief assigned):

- `<scratch>/docs-check.log`
- `<scratch>/alone-1.log`
- `<scratch>/alone-2.log`
- `<scratch>/alone-3.log`
- `<scratch>/alone-4.log`
- `<scratch>/memory-5.log`
- `<scratch>/suite.log`
- `<scratch>/suite-final.log`
- `<scratch>/fmt.log`
- `<scratch>/clippy.log`

## 7. Findings block

```findings
- file: adapters/inference-elevenlabs/src/agent_duplex.rs
  line: 512
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the URL leaves the model at Target::dial before the request is built and before the handshake is first polled, so an open that made no request or was cancelled first spends it with nothing dialled
- file: adapters/inference-elevenlabs/src/agent_duplex.rs
  line: 34
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the module doc says every later open of a spent model fails as SignedUrlSpent, but one whose configuration the binding refuses fails as OpenError::Unsupported with open_error() None
- file: adapters/inference-elevenlabs/tests/agent_duplex.rs
  line: 661
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the timeout-and-cancel case observes the next open's refusal and the connection count only, so no committed test pins that a timed-out or cancelled handshake releases and zeroizes the buffer
```

