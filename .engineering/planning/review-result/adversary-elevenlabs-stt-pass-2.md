---
format: aep.planning-md/3
id: review-result:adversary-elevenlabs-stt-pass-2
kind: review-result
status: active
title: Adversary pass 2 on the ElevenLabs realtime STT adapter
relations:
- reviews: story:elevenlabs-realtime-stt-adapter
revision: 2
---
```
unit: story:elevenlabs-realtime-stt-adapter, commit fc0b0b0d plus untracked test files (worktree impl-elevenlabs-realtime-stt-adapter)
verdict: CONFIRMED
cases: executed 145→147, red 0
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 8 paths, all under the assigned adversary2 scratch directory (one is a 510 MB source copy with its build)
needs-coordinator: no
```

I could not break the correction. My two findings are gaps in the suite, not defects: two lines the correction added had no test that would fail if they were removed. I added a green case for each and proved it on a mutated copy. The only red cases in the suite belong to the security reviewer.

## 1. Diff
`git --no-pager diff --stat` is empty: no tracked file changed. `git status --short`:
```
?? adapters/inference-elevenlabs/tests/adversary2_u1_opening.rs        (mine)
?? adapters/inference-elevenlabs/tests/security2_u1_end_deadline.rs    (security reviewer's, not touched)
?? adapters/inference-elevenlabs/tests/security2_u1_opening.rs         (security reviewer's, not touched)
```
I changed no implementation file. I ran `rustfmt --edition 2024` on my own file only.

## 2. Cases added
Both are in `adapters/inference-elevenlabs/tests/adversary2_u1_opening.rs`. Both are green on fc0b0b0d. Each was run alone first, before the suite (`cargo test -p inference-elevenlabs --all-features --test adversary2_u1_opening`):
```
test a_credential_that_never_resolves_fails_as_connect_within_the_connect_timeout ... ok
test a_handshake_slower_than_the_read_timeout_that_succeeds_after_the_end_of_input_keeps_its_transcript ... ok
test result: ok. 2 passed; 0 failed
```
To show each case catches a defect, I built a copy of fc0b0b0d in scratch, mutated it there, and ran the case against it. The worktree was never mutated.

- **A. `a_handshake_slower_than_the_read_timeout_that_succeeds_after_the_end_of_input_keeps_its_transcript`**
  - **Setup:** the server answers the WebSocket upgrade 1.2 s after the client connects, after input has ended. The read timeout is 300 ms. The server then answers the commit at once.
  - **Asserts:** no failure, the events `["hypothesis eins", "commit eins", "usage"]`, and a final usage record.
  - **Pins:** `recogniser.rs:466`, where opening the connection restarts the read timeout. This is the second half of security pass-1 finding 1, which no case covered.
  - **Mutant 1:** `Inbound::Opened | Inbound::Message(_)` becomes `Inbound::Message(_)`. The case goes red:
    ```
    panicked at adapters/inference-elevenlabs/tests/adversary2_u1_opening.rs:100:5:
    the connection opened 1.2 s after the client connected and the commit was answered at once, yet the recognition failed; events ["usage"]
      left: Some(Stalled(300ms))
     right: None
    ```
- **B. `a_credential_that_never_resolves_fails_as_connect_within_the_connect_timeout`**
  - **Setup:** a `CredentialSource` whose future never resolves. Connect timeout 500 ms.
  - **Asserts:** the recognition fails as `Connect`, in under 2 s, with a non-final usage record. A 5 s watchdog catches a hang.
  - **Pins:** the outer connect timeout that `drive` wraps around `Connection::open` (`recogniser.rs:718-719`). The handshake timeout inside `Connection::open` only starts after the credential resolves, so nothing else bounds this wait.
  - **Mutant 2:** `connect_timeout` becomes `Duration::MAX` at `recogniser.rs:719`. The case goes red:
    ```
    panicked at adapters/inference-elevenlabs/tests/adversary2_u1_opening.rs:152:10:
    a credential that never resolves held the recognition for 5 s, ten times the 500 ms connect timeout: Timeout
    ```
- **Each case catches only its own mutant.** Case B stayed green under mutant 1, and case A stayed green under mutant 2.

## 3. Suite run (after the cases existed)
`cargo test -p inference-elevenlabs --all-features --no-fail-fast` exited 101:
```
tests/adversary2_u1_opening.rs      test result: ok. 2 passed; 0 failed
tests/adversary_u1_end_of_input.rs  test result: ok. 3 passed; 0 failed
tests/security_u1_stt.rs            test result: ok. 4 passed; 0 failed
tests/stt.rs                        test result: ok. 15 passed; 0 failed
tests/stt_conformance.rs            test result: ok. 8 passed; 0 failed
tests/security2_u1_end_deadline.rs  test result: FAILED. 0 passed; 2 failed   (not mine)
tests/security2_u1_opening.rs       test result: FAILED. 2 passed; 1 failed   (not mine)
error: 2 targets failed:
exit 101
total: passed 144, failed 3, ignored 3, executed 147
```
- **Where 145 comes from:** 147 minus my target's 2, from this same run.
- **Without the security reviewer's 5 cases as well:** 140.
- **The 3 red cases are the security reviewer's.** They cover a server streaming messages that never answer the commit, and a credential lookup that blocks its own thread. I had the first as a judgement note and dropped it as a duplicate of their red case.

## 4. Findings
They cover fc0b0b0d plus the untracked test files.

1. **Opening the connection restarts the read timeout, and nothing tested it.**
   - **What was measured:** `recogniser.rs:466`. With `Opened` removed, the full suite stays green and case A goes red.
   - **What reaches it:** a handshake that finishes after input ended and takes longer than the read timeout. With the defaults (10 s each) that is rare, but likely when the read timeout is set short.
   - CONFIRMED, introduced, note. Case A closes the gap.
2. **The outer connect timeout is the only bound on credential resolution, and nothing tested it.**
   - **What was measured:** `recogniser.rs:718`. With that bound removed, the full suite stays green and case B hangs.
   - **What reaches it:** `KeychainSource`, waiting on an unlock prompt (it uses `spawn_blocking`, so the timeout can fire), or any async source that stalls.
   - CONFIRMED, introduced, note. Case B closes the gap.

## 5. Attacked and could not break
- **Pass-1 cases:** S1–S4 and A1, A2, A4 hold. Their pass-1 cases are green, and the new `stt.rs` cases for answered and unanswered commits are green.
- **Handshake still pending at end of input:** the handle waits on the thread with no deadline of its own; the outer timeout bounds that wait. Its outcome is always the first thing the handle hears: `Opened` is the first item in the queue.
- **`checked_add` on both deadlines:** with an overflowing read timeout, `settle` and `due` choose correctly between the VAD quiet period and the read timeout.
- **`quiet()` with `outstanding > 0`:** manual and VAD counting, the drive thread already dead when the end commit is sent, and a failure queued before end of input.
- **Blocking `inbound.send(Opened)`:** the queue is always empty at that point, and a dropped handle closes the connection.
- **Timeout of `Duration::MAX` on the recogniser path:** tokio's `timeout` takes it as no deadline, so nothing panics.
- **Messages queued before end of input:** consuming them after end of input only lengthens the deadline.
- **Tests that cannot fail:** the new and changed cases in `stt.rs` assert literal events and values; none has one of the five shapes.
- **4 MiB message limit on the TTS path:** I found no realistic TTS message that large, because audio streams in many messages per generation.

## 6. Paths written outside the worktree
- `<scratch>/wave-p1/U1/adversary2/` (directory)
- `<scratch>/wave-p1/U1/adversary2/alone.log`
- `<scratch>/wave-p1/U1/adversary2/suite.log`
- `<scratch>/wave-p1/U1/adversary2/mutant1.diff`
- `<scratch>/wave-p1/U1/adversary2/mutant1.log`
- `<scratch>/wave-p1/U1/adversary2/mutant2.diff`
- `<scratch>/wave-p1/U1/adversary2/mutant2.log`
- `<scratch>/wave-p1/U1/adversary2/copy/` — the `git archive` of fc0b0b0d with mutant 2 applied, plus its own `target/`. 510 MB in total; safe to delete.

The worktree build went to its own `target/`. I held lease `adversary2-u1-stt` and released it.

## 7. Findings block
```findings
- file: adapters/inference-elevenlabs/src/recogniser.rs
  line: 466
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: restarting the read timeout when the connection opens was pinned by no case, so dropping Opened from the match left the suite green while a slow successful handshake failed as Stalled; adversary2_u1_opening case A now pins it
- file: adapters/inference-elevenlabs/src/recogniser.rs
  line: 718
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the connect timeout drive wraps around Connection::open is the only bound on credential resolution and was pinned by no case, so removing it left the suite green while a never-resolving credential hung the caller; adversary2_u1_opening case B now pins it
```
