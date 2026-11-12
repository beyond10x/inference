---
format: aep.planning-md/3
id: review-result:adversary-elevenlabs-stt-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the ElevenLabs realtime STT adapter
relations:
- reviews: story:elevenlabs-realtime-stt-adapter
revision: 2
---
```
unit: story:elevenlabs-realtime-stt-adapter, commit 141a2f20 plus the untracked test file below (worktree impl-elevenlabs-realtime-stt-adapter)
verdict: NEEDS-CHANGE
cases: executed 136→139, red 3
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 6 paths, all in the assigned adversary scratch directory
needs-coordinator: whether a server that never answers is a failure is a decision the story did not settle (finding 2); the 136 count includes the security reviewer's 4 cases from security_u1_stt.rs
```

## 1. Diff
`git --no-pager diff --stat` is empty because my only change is one new, untracked file. `git status --short`:
```
?? adapters/inference-elevenlabs/tests/adversary_u1_end_of_input.rs   (mine)
?? adapters/inference-elevenlabs/tests/security_u1_stt.rs             (security reviewer's, not touched)
```
I changed no implementation file. I formatted my own file with `rustfmt --edition 2024` only.

## 2. Cases added
All three are in `adapters/inference-elevenlabs/tests/adversary_u1_end_of_input.rs`, behind `#![cfg(feature = "stt")]`. All three are red now. Each was first run alone, before any suite run. Those first runs were before rustfmt, so their line numbers 144 and 183 are 146 and 185 now.

- **A. `a_server_that_keeps_talking_after_the_end_of_input_is_not_reported_as_stalled`**
  - Setup: manual commit, read timeout 2 s. After the end-of-input commit the server sends a partial every 700 ms, then its committed transcript at about 2.8 s.
  - Asserts: no failure, and the commit is delivered.
  - Red output:
    ```
    panicked at .../adversary_u1_end_of_input.rs:95:5:
    the server sent a message at most 700 ms apart, never silent for the 2 s read timeout; events ["hypothesis eins", "revision eins zwei", "revision eins zwei drei", "usage"]
      left: Some(Stalled(2s))
     right: None
    ```
- **B. `a_server_that_never_says_anything_does_not_end_the_recognition_as_a_success`**
  - Setup: the server accepts the handshake and then sends nothing, not even `session_started`. One second of audio is sent, and the end commit reaches the server.
  - Asserts: the recognition fails, and its usage record is not final.
  - Red output:
    ```
    panicked at .../adversary_u1_end_of_input.rs:144:5:
    one second of audio sent, its commit never answered and no message at all from the server, yet no failure after 800.507923ms; events ["usage"], usage Some((Some(1000), true))
    ```
- **C. `with_vad_the_transcript_answering_the_end_of_input_commit_is_not_dropped`**
  - Setup: VAD commit. A VAD commit still on its way answers for the end commit, and the transcript of the tail audio arrives 700 ms later.
  - Asserts: both spans are committed.
  - Red output:
    ```
    panicked at .../adversary_u1_end_of_input.rs:183:5:
      left: ["hypothesis eins", "commit eins", "usage"]
     right: ["hypothesis eins", "commit eins", "hypothesis zwei", "commit zwei", "usage"]
    ```

## 3. Suite run (after the cases existed)
The gate `cargo test -p inference-elevenlabs --all-features` exited 101. It stopped at the first failing target, `adversary_u1_end_of_input`, with 0 passed and 3 failed. I re-ran it with `--no-fail-fast` to get the full count:
```
tests/adversary_u1_end_of_input.rs  test result: FAILED. 0 passed; 3 failed   (panics at :95, :146, :185)
tests/security_u1_stt.rs            test result: FAILED. 0 passed; 4 failed   (not mine)
tests/stt.rs                        test result: ok. 14 passed
tests/stt_conformance.rs            test result: ok. 8 passed
error: 2 targets failed:
exit 101
total: passed 132, failed 7, ignored 3, executed 139
```
- **Where 136 comes from:** 139 minus my target's 3, all from this one run.
- **Without the security reviewer's file:** 132.
- **Full logs:** in the scratch paths listed in part 6.

## 4. Findings
They cover 141a2f20 plus the untracked test files above.

1. **A server still sending messages is reported as `Stalled`.**
   - **What was measured:** case A at `tests/adversary_u1_end_of_input.rs:95`, exit 101.
   - **The defect:** `recogniser.rs:410` sets one deadline at the end-of-input commit, and messages do not extend it (`:422-433`). When that deadline passes with a span open, the recognition fails as `Stalled` (`:477-479`). But `error.rs:92-94` documents `Stalled` as "No message arrived for the read timeout", and it displays as "no server message within 2s". Here the last message was about 600 ms old.
   - **What reaches it:** any transcription that takes longer than `read_timeout` (default 10 s, settable with `Endpoint::with_read_timeout`) after the end of input while partials keep arriving. Examples are audio pushed faster than real time or a slow server. ElevenLabs' own guide waits per message (`asyncio.wait_for(ws.recv(), timeout=10.0)`).
   - **Fix to name:** push the deadline forward on every message, as `heard_at` already tracks.
   - **Other:** Deepgram's `recogniser.rs:366-380` has the same one-deadline shape.
   - NEEDS-CHANGE, introduced.
2. **A server that never answers ends the recognition as a success.**
   - **What was measured:** case B at `:146`.
   - **The defect:** `recogniser.rs:477-483` treats silence past the deadline with no span open as a clean end, even when the end commit is still unanswered and the server never sent `session_started`. The result is a final usage record and no transcript, which a voice agent reads as "the caller said nothing". This is the implementor's stated decision, but it hides an outage.
   - **What reaches it:** a hung server or a broken session on the hosted endpoint.
   - **Fix to name:** with `manual`, an unanswered commit (`outstanding > 0`) at the deadline should be `Stalled`. At least a session that never sent a message should be.
   - **Note:** the existing test `stt.rs:319` pins the clean end for a server that did send `session_started`. Case B is the narrower state where it sent nothing at all.
   - NEEDS-CHANGE, introduced.
3. **With VAD, the tail transcript is dropped after 500 ms of quiet.**
   - **What was measured:** case C at `:185`.
   - **The defect:** at `recogniser.rs:417-432` and `:43`, a VAD commit already on its way when input ends counts as the answer to the end commit. The binding then ends after 500 ms (`VAD_END_QUIET`) without a message, and a tail transcript arriving later is lost while the recognition reports success. `stt_protocol.rs:72-74` documents this as a known loss.
   - **What reaches it:** VAD (which the live test uses), a VAD commit in flight at end of input, and a tail answer later than 500 ms with no partial before it. The window is narrow, and the timing is constructed.
   - CONFIRMED, introduced, note.
4. **The runtime error message says "synthesis" for a recognition.**
   - **What was measured:** `error.rs:185` displays `Runtime` as "the synthesis's runtime failed".
   - **What reaches it:** `recogniser.rs:526-528` (the worker thread stopped) and `:619/:623` (runtime or thread start failed). This is judgement, with no case.
   - CONFIRMED, introduced, note.

## 5. Attacked and could not break
- **Committed messages in the order plain then with-timestamps:** this matches ElevenLabs' own examples, where `committed_transcript_with_timestamps` comes last and ends the loop, so the "only with-timestamps commits" decision holds for that order.
- **`committed_transcript` revising the open span:** consistent with the `stt-ulaw` fixture.
- **Span timing from words:** spacing and untimed words are left out correctly.
- **Frame checks:** a frame in another format or with a wrong byte length is refused before connecting.
- **Usage on failure:** counts taken frames, as the contract defines it (`inference-stt/src/lib.rs:56`).
- **Configuration validation and URL encoding:** no break.
- **Manual-mode commit counting:** no break.
- **Tests that cannot fail:** the new tests assert literal sequences and values; I found none of the five shapes.
- **Mutant left open:** the suite has no timing check on the manual early-end path (`recogniser.rs:466-469`). If it were dropped, recognitions would end clean but `read_timeout` late. I did not prove this on a mutated copy.

## 6. Paths written outside the worktree
- `<scratch>/wave-p1/U1/adversary/build.log`
- `<scratch>/wave-p1/U1/adversary/alone.log`
- `<scratch>/wave-p1/U1/adversary/suite.log`
- `<scratch>/wave-p1/U1/adversary/suite-nff.log`
- `<scratch>/wave-p1/U1/adversary/docs/realtime-guide.md` (the public ElevenLabs guide, fetched without a key)
- The `docs/` directory itself.

I took and released the worktree lease `adversary-u1-stt`.

## 7. Findings block
```findings
- file: adapters/inference-elevenlabs/src/recogniser.rs
  line: 410
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the end-of-input deadline is never extended by arriving messages, so a server streaming partials every 700 ms is failed as Stalled, which error.rs:92 documents as no message for the read timeout
- file: adapters/inference-elevenlabs/src/recogniser.rs
  line: 477
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a server that accepts the handshake and never sends anything ends the recognition as a success with a final usage record and no transcript instead of a stall
- file: adapters/inference-elevenlabs/src/recogniser.rs
  line: 432
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: with vad, a tail transcript arriving more than 500 ms after an in-flight VAD commit answered for the end-of-input commit is dropped while the recognition reports success
- file: adapters/inference-elevenlabs/src/error.rs
  line: 185
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Runtime displays as the synthesis's runtime failed although recognitions now raise it
```
