---
format: aep.planning-md/3
id: review-result:adversary-elevenlabs-tts-cancel-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the ElevenLabs TTS provider-side cancel
relations:
- reviews: story:elevenlabs-tts-provider-cancel
revision: 2
---
unit: story:elevenlabs-tts-provider-cancel, commit bd662085 plus 2 untracked adversary test files
verdict: NEEDS-CHANGE
cases: executed 200→202, red 2
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 4 paths
needs-coordinator: whether ElevenLabs stops generating a flushed context when it gets `close_context` (only a billed run can tell, and the fix depends on the answer)

**1. Diff stat.** `git --no-pager diff --stat` prints nothing, because both new files are untracked. `git status --short`:
```
?? adapters/inference-elevenlabs/tests/adversary_u2_no_audio_context.rs
?? adapters/inference-elevenlabs/tests/adversary_u2_quiet_cut.rs
```
Both are test files. I changed no implementation file and no existing test.

**2. Cases added.** Both are red now. Each was run on its own before the suite ran.

- `adversary_u2_quiet_cut.rs`: it uses the unit's own fixture server, which stops generating a context when it gets `close_context`. The server pauses 300 ms between audio chunks and the quiet is the default 250 ms. The case expects all 4 chunks and no cut.
  ```
  panicked at adapters/inference-elevenlabs/tests/adversary_u2_quiet_cut.rs:52:9:
  assertion `left == right` failed: the binding closed the context while the server was still generating it
    left: [("s0", 1)]
   right: []
  test result: FAILED. 0 passed; 1 failed; ... finished in 0.26s
  exit 101
  ```
  The segment still ends as Synthesized, with 1 of its 4 chunks and no error. The extra "did not finish within 20s" line comes from `support::within` when the inner thread panics; it is not a timeout.
- `adversary_u2_no_audio_context.rs`: a loopback server that follows the unit's own `Script::Serve` model, where a flushed context ends only when it is closed. It sends no audio for the text "…". The case offers ["…", "After the pause."] and expects both segments to end Synthesized.
  ```
  panicked at adapters/inference-elevenlabs/tests/adversary_u2_no_audio_context.rs:127:9:
  after 2.009097788s: Some(Stalled(2s)); closes the server saw: []
  test result: FAILED. 0 passed; 1 failed; ... finished in 2.02s
  exit 101
  ```

**3. Suite run** (after the cases existed): `cargo test -p inference-elevenlabs -p inference-tts --all-features --no-fail-fast`
```
error: 2 targets failed:
    `-p inference-elevenlabs --test adversary_u2_no_audio_context`
    `-p inference-elevenlabs --test adversary_u2_quiet_cut`
exit 101
```
202 cases ran: 200 passed, 2 failed, and every other target was ok. The "before" figure of 200 comes from this same run with my two binaries (1 case each) left out of the count, not from a separate run.

**4. Findings** (covering bd662085)

| # | file:line | finding | verdict | origin |
|---|---|---|---|---|
| 1 | `adapters/inference-elevenlabs/src/synthesiser.rs:443` | A pause longer than `context_quiet` in the middle of a segment makes `close_live` send the close. In the unit's own server model, which is the story's premise that a close stops generation, that cuts the segment short. The segment still ends as Synthesized with no failure. The suite avoids this on purpose: `tests/support/mod.rs:88-91` says QUIET is "longer than any pace a test sets". The design needs the close to stop generation at a cancel but not at a quiet close, and the docs cannot give both. Either the quiet close cuts audio, or "will continue flushing" holds and the cancel close stops nothing. | NEEDS-CHANGE | introduced |
| 2 | `adapters/inference-elevenlabs/src/synthesiser.rs:437` | The quiet only starts once a message of the context arrives. A context the endpoint answers with no audio therefore never gets a close, waits out `read_timeout` (10 s by default), and fails the whole synthesis, losing every later segment. The base sent the close together with the text (removed at old `send_context`, `connection.send(close)`), so such a segment used to complete with no frames. | CONFIRMED | introduced |

What reaches each one:
- **Finding 1:** a real server that pauses more than 250 ms between chunks, for example a slower model than flash v2.5 or network jitter, combined with close stopping generation. I did not measure either on the real endpoint. The fixture's "cut" semantics are the unit's own claim.
- **Finding 2:** any caller can offer such text; segments that are punctuation or emoji only are plausible from LLM segmentation. Not shown on the real endpoint: whether ElevenLabs sends nothing for such text until the close. That behaviour is the unit's own model in `Script::Serve`.

Fix direction, not applied:
- **Finding 2:** start the quiet when the text is sent, or keep a separate bound for a first message that never comes.
- **Finding 1:** depends on the coordinator question in the header. If a close stops generation, the quiet close must not fire while audio may still be coming. If it does not, the cancel claim in `lib.rs` and the CHANGELOG has to be withdrawn.

Origin is taken from the diff, which adds both lines (the quiet timer is new, and the close that went with the text was removed). I did not run anything against the base. Both cases need `with_context_quiet`, which does not exist at the base, so they cannot compile there.

**5. Attacked and not broken**
- A cancel sends `close_context` within `CLOSE_BOUND`; a failed write drops the connection and clears `cancelled`.
- Frames of a cancelled context are dropped and do not move the read deadline or the quiet.
- A cancel after a quiet close sends no second close and the server records no violation.
- The reopen-once path between segments behaves as before: `heard` and `reopened` gate it, and the reopen path is unchanged.
- Time to first audio is measured from the offer through `audio`/`complete`, so it holds for every synthesiser through `SynthesisOutput`.
- The latency report column takes the first segment in first-frame order, and native rows show n/a.

**6. Paths written outside the worktree**
- `<scratch>/wave-p1/U2/adversary/red-quiet-cut.log`
- `<scratch>/wave-p1/U2/adversary/red-no-audio.log`
- `<scratch>/wave-p1/U2/adversary/suite.log`
- `<scratch>/wave-p1/U2/adversary/` (the directory itself, created)

The build went to the worktree's own `target/`.

**7. Findings block**
```findings
- file: adapters/inference-elevenlabs/src/synthesiser.rs
  line: 443
  category: contract-drift
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: under the unit's own close-stops-generation fixture model, a mid-segment pause longer than context_quiet triggers close_live and cuts the segment to 1 of 4 chunks while it still completes as Synthesized with no failure (tests/adversary_u2_quiet_cut.rs:52)
- file: adapters/inference-elevenlabs/src/synthesiser.rs
  line: 437
  category: boundary
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the quiet only arms after a first message, so a context the endpoint answers with no audio is never closed, stalls for read_timeout and fails the whole synthesis, where the base's close-with-text completed it (tests/adversary_u2_no_audio_context.rs:127)
```
