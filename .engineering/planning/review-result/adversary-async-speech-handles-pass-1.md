---
format: aep.planning-md/3
id: review-result:adversary-async-speech-handles-pass-1
kind: review-result
status: active
title: Adversary pass 1 on story:async-speech-handles
relations:
- reviews: story:async-speech-handles
revision: 1
---
unit: story:async-speech-handles, commit 63a40480 (findings cover 63a40480; the adversary tests are commit be5caa16 on top of it)
verdict: NEEDS-CHANGE
cases: executed 498→504, red 3
origin: introduced 4 / pre-existing 1 / undecided 0
wrote-outside-worktree: 10 paths, all in the assigned scratch directory
needs-coordinator: whether the STT contract's "None while input is open" should become `Pending` plus a wake for hosted recognisers (finding 2). That is a change to the contract's meaning, not only an implementation fix.

**Verdict:** NEEDS-CHANGE. The async form never delivers server events on the OpenAI Realtime and GPT-Live duplex sessions, and the ElevenLabs recogniser's async form ends at once mid-input instead of waiting. Three red cases, each with a green control on the same replay.

**1. `git --no-pager diff --stat 63a40480 HEAD`**
```
 .../tests/adversary_async_mid_input.rs             |  90 ++++++
 .../tests/adversary_async_duplex_reads.rs          | 345 +++++++++++++++++++++
 2 files changed, 435 insertions(+)
```
Only test files changed. No implementation file was touched.

**2. Cases added**

`adapters/inference-openai/tests/adversary_async_duplex_reads.rs` (features `duplex`, `gpt-live`):
- `control_the_realtime_replay_completes_through_the_blocking_form`: green. The replay completes through `next_event`.
- `the_realtime_session_delivers_the_servers_response_through_the_async_form`: red now. Asserts `event()` delivers the server's response within 2 s on a current-thread runtime.
- `control_the_gpt_live_replay_completes_through_the_blocking_form`: green.
- `the_gpt_live_session_delivers_the_servers_response_through_the_async_form`: red now.

Red output from the run of that file alone:
```
panicked at adapters/inference-openai/tests/adversary_async_duplex_reads.rs:248:5:
the server sent response.created, a delta and response.done; the async form delivered within 2s only []
panicked at adapters/inference-openai/tests/adversary_async_duplex_reads.rs:342:5:
the server sent a voiced frame and a silence gap; the async form delivered within 2s only ["InputAccepted", "InputAccepted", "InputAccepted"]
test result: FAILED. 2 passed; 2 failed
```
The three `InputAccepted` events come from the session's own record of the audio the test sent; nothing from the server was read.

`adapters/inference-elevenlabs/tests/adversary_async_mid_input.rs` (feature `stt`):
- `control_the_partial_arrives_after_the_async_forms_none_without_more_input`: green. The first `event()` returns `None`; polled again with no more input offered, the partial is there.
- `the_async_form_waits_for_the_partial_the_server_sends_while_input_is_open`: red now.
```
panicked at adapters/inference-elevenlabs/tests/adversary_async_mid_input.rs:47:5:
assertion `left == right` failed: the server answers the first chunk with a partial; awaiting the next event while input is open should wait for it, not end at once
  left: Ok(None)
 right: Ok(Some("hypothesis hallo"))
```
(At the commit the assertion is at line 82, after the control was added and the file reformatted.)

**3. Suite run, after the cases existed** (`CARGO_INCREMENTAL=0`, `cargo +1.100.0`)
- `cargo test -p inference-openai --features duplex,gpt-live,speech --no-fail-fast`: 342 executed, 340 passed, 2 failed (`adversary_async_duplex_reads`). Exit 101. Without my file: 338.
- `cargo test -p inference-elevenlabs --features stt,duplex --no-fail-fast`: 162 executed, 161 passed, 1 failed (`adversary_async_mid_input`). Exit 101. Without my file: 160. The unit's own `async_handles` passed 3 of 3.
- `cargo clippy` on both crates, all targets, those features, `-D warnings`: exit 0.
- `cargo fmt --all --check`: exit 0, after I formatted my own file.

**4. Findings** (all cover 63a40480)

1. **The OpenAI Realtime and GPT-Live duplex sessions have no working async form.**
   - Where: `crates/inference-duplex/src/contract.rs:468` (the trait's default), reached through `adapters/inference-openai/src/duplex.rs:1299` and `adapters/inference-openai/src/live_duplex.rs:1254`.
   - Measured: the two red cases above.
   - Cause: both files are cited in the story's scope, but neither overrides `poll_event`. The default calls `next_event(Duration::ZERO)`, and on a zero wait both bindings return before reading their connection. So `event()` keeps waking itself and never delivers a server event. A session waiting for a response would then fail as Stalled once its idle limit passes.
   - Who reaches it: any async caller of either binding. The Planned scope says every adapter implementing the contracts provides the async form.
   - Fix (not applied): override `poll_event` in both bindings, as the Deepgram and ElevenLabs agents do.
   - NEEDS-CHANGE, introduced.

2. **The ElevenLabs recogniser's async form ends at once while input is open.**
   - Where: `adapters/inference-elevenlabs/src/recogniser.rs:503`. With nothing queued before the end of input, it returns `Ready(None)` and registers no waker.
   - Measured: the red case above. The green control shows the partial arrives with no further input, so the `None` did not mean what the contract says it means.
   - Who reaches it: a phone agent awaiting partial transcripts while the caller speaks. The story's scope wants the async form "woken by the adapter when an event arrives rather than polled".
   - The same shape is at `adapters/inference-deepgram/src/recogniser.rs:406`, read only, not run.
   - Fix (not applied): return `Pending` and register the waker mid-input for hosted recognisers. This changes what `None` means in the contract, hence needs-coordinator.
   - NEEDS-CHANGE, introduced.

3. **Three more adapters cited in the scope keep the default; judgement, no failing case.**
   - The AWS Nova Sonic duplex (`adapters/inference-aws/src/duplex.rs:1318`) gets the default. It does deliver, because a zero-wait `recv_timeout` takes what has arrived, but it wakes itself on every poll, so each waiting session busy-spins a CPU instead of being woken.
   - The sherpa-onnx and whispercpp STT adapters (`adapters/inference-sherpa-onnx/src/stt.rs`, `adapters/inference-whispercpp/src/stt.rs`) get the default `Poll::Ready(self.next())`, which blocks the runtime thread for the whole recognition.
   - The design decided at scoping says the adapters named in the scope override the default with a non-blocking form; these do not.
   - Who reaches it: async callers of these adapters. I wrote no case: it needs AWS's HTTP/2 transport or model checkpoints.
   - CONFIRMED, introduced.

4. **The STT conformance through the async form leaves out `backend_failure`.**
   - Where: `crates/inference-testkit/tests/async_handles.rs`, `the_stt_conformance_passes_through_the_async_form_for_every_mock_configuration`, against `crates/inference-testkit/src/stt/conformance.rs:547`.
   - It runs 6 of the 7 procedures; acceptance says the conformance runs once more through the async form.
   - CONFIRMED, introduced.

5. **Dropping an idle synthesis on an async worker blocks that thread.**
   - Where: `adapters/inference-elevenlabs/src/synthesiser.rs:776` (the same in Cartesia's `Drop`).
   - With a connection open and no exchange in flight, `Drop` starts a close and blocks for up to `CLOSE_BOUND` (1 s). The agent sessions document that dropping inside a runtime does not wait; the synthesisers neither document nor avoid it.
   - Pre-existing: the base `Drop` (4524fd65, `synthesiser.rs:577`) blocked the same way through `run`.
   - Who reaches it: a caller dropping the handle between segments, for example after a barge-in.
   - INFEASIBLE here (out of this story's scope), pre-existing.

**5. Attacked and not broken**
- Acceptance 4: `cargo tree -p inference-tts -p inference-stt -p inference-duplex -e normal` has no `tokio`.
- Recogniser waker order (register, then `try_recv`; wake after each send and on drop): no lost wakeup found by reading.
- ElevenLabs agent `poll_event` alarm loop: a past due time is replaced by `now + limit`, so it does not spin; a zero idle limit is refused at open.
- ElevenLabs and OpenAI speech discard while an exchange is in flight: the cancelled context's messages are dropped, the OpenAI exchange is aborted, and no event of a discarded segment reaches the caller.
- The ElevenLabs acceptance tests hold the server's answer for 150 ms, so a blocking poll would push the ticker past its 50 ms bound; that guard can fail.
- `LateSynthesis::pull`: the blocking and async forms hold back the same frames.

**6. Paths written outside the worktree**
All under `<scratch>/p2/wave3/adversary/`:
- `openai-case.log`
- `openai-case2.log`
- `elevenlabs-case.log`
- `suite-openai.log`
- `suite-elevenlabs.log`
- `fmt.log`
- `clippy.log`
- `cases.done`
- `run-cases.sh`
- `run-suite.sh`
- `commit-msg.txt`
- `suite.done` (the suite script writes it last)

The scratch directory itself was created in this pass.

Commit: be5caa16. `target/` was 3.0 GB and is deleted; it did not exist before this pass. I released my worktree lease.

**7. Findings block**
```findings
- file: crates/inference-duplex/src/contract.rs
  line: 468
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the OpenAI Realtime and GPT-Live sessions keep the default poll_event, whose zero-wait next_event never reads their connection, so event() never delivers a server event (adversary_async_duplex_reads.rs, two red cases)
- file: adapters/inference-elevenlabs/src/recogniser.rs
  line: 503
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: while input is open the recogniser's async form returns Ready(None) with no waker registered, so awaiting a partial ends at once and the caller must poll on a timer (adversary_async_mid_input.rs)
- file: adapters/inference-aws/src/duplex.rs
  line: 1318
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: scope-cited AWS duplex, sherpa-onnx STT and whispercpp STT keep the trait defaults, which busy-poll (AWS) or block the runtime thread (local STT) instead of the non-blocking override the scoping design requires
- file: crates/inference-testkit/tests/async_handles.rs
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the STT conformance rerun through the async form omits backend_failure, one of the seven procedures in inference_testkit::stt::conformance
- file: adapters/inference-elevenlabs/src/synthesiser.rs
  line: 776
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: pre-existing
  message: dropping an idle synthesis on an async worker blocks that thread for up to CLOSE_BOUND while the close completes, undocumented, as the base Drop already did
```