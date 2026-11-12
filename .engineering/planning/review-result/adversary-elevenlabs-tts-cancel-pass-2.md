---
format: aep.planning-md/3
id: review-result:adversary-elevenlabs-tts-cancel-pass-2
kind: review-result
status: active
title: Adversary pass 2 on the ElevenLabs TTS provider-side cancel
relations:
- reviews: story:elevenlabs-tts-provider-cancel
revision: 3
---
```
unit: story:elevenlabs-tts-provider-cancel, impl/elevenlabs-tts-provider-cancel at 7f1c4237 (base 016e083d) plus my one untracked test file
verdict: CONFIRMED (notes and warnings only; no red behaviour case)
cases: executed 153→158, red 0 of mine (the suite has 3 red cases, all in the security reviewer's security_u2_*.rs files)
origin: introduced 6 / pre-existing 1 / undecided 0
wrote-outside-worktree: 9 paths, all under wave-p1/U2/review-2/
needs-coordinator: the story's title, summary and Acceptance 1 still promise the withdrawn provider-side cancel
```

**1. `git --no-pager diff --stat`**

The tracked diff is empty. No tracked file changed, and no implementation file changed. The untracked files are:
- `adapters/inference-elevenlabs/tests/adversary2_u2_residency_env.rs` (mine)
- `security_u2_changelog_breaking.rs` and `security_u2_residency_echo.rs` (the security reviewer's, not mine)

**2. The case I added**

File: `<worktrees>/impl-elevenlabs-tts-provider-cancel/adapters/inference-elevenlabs/tests/adversary2_u2_residency_env.rs`

- It runs its own test binary again as a child process, so the variable is set on the child; the workspace forbids `unsafe`, so `set_var` is not an option.
- It sets `ELEVENLABS_RESIDENCY` to unset, `global`, `us`, `eu`, `in`, `sg` and `europe`, and asserts the exact host URL, or `ConfigError::Residency`, that `Endpoint::from_env` returns for each.
- It is a guard against a mutant the suite misses, so it is **green now**.
- My first draft had a parsing bug in the test itself (it looked for `RESULT ` at the start of the line). I fixed that before any run counted.

The case alone, at HEAD: `2 passed; 0 failed`, exit 0.

To show it catches something, I ran it against a mutated copy in scratch, not in the worktree. The mutation is in `mutant.diff`:
```
-        match std::env::var(RESIDENCY_VARIABLE) {
+        match std::env::var("ELEVENLABS_REGION") {
```
Results against the mutant:
- The two existing unit tests in `config.rs`: `2 passed`, exit 0. They miss the mutant.
- My case: red, exit 101:
```
assertion `left == right` failed: ELEVENLABS_RESIDENCY=Some("us")
  left: "ok wss://api.elevenlabs.io"
 right: "ok wss://api.us.elevenlabs.io"
```

**3. Suite run**

Command: `cargo test -p inference-elevenlabs --all-features --no-fail-fast`. Result: 155 passed, 3 failed, 5 ignored, exit 101. Full output is in `review-2/suite.log`.
- The 3 failures are all in the security reviewer's files: `security_u2_changelog_breaking` (1 case) and `security_u2_residency_echo` (2 cases).
- The "before" count of 153 comes from the implementor's run (`scratch/gate-c1b/test-elevenlabs.log`: 153 passed, 4 ignored).
- The +5 executed cases are my 2 plus the security reviewer's 3.

**4. Findings** (all cover 7f1c4237)

| # | file:line | verdict | origin | finding | what reaches it |
|---|---|---|---|---|---|
| F1 | `adapters/inference-elevenlabs/src/config.rs:124` | CONFIRMED | introduced | No test that runs without feature `live` calls `Endpoint::from_env`. The mutant above passed the unit tests. Fixed by adding my case. | The example `live`, `stt_live` and `tts_cancel_live`, which is how the EU key reaches its host. |
| F2 | `adapters/inference-elevenlabs/src/lib.rs:37`, `CHANGELOG.md:34` | CONFIRMED | introduced | Both say 99% of the audio (22.5 of 22.7 s) came after the close. The probe log says 22158 ms came after the close, which is 97.7% of P3. The 22530 ms figure is all of P2's audio, including 371 ms from before the close. The correction-1b brief contains the same misreading. Fix: "97.7% (22.2 of 22.7 s)". | Readers of the docs only. The conclusion still holds. |
| F3 | `adapters/inference-elevenlabs/tests/support/mod.rs:17` | CONFIRMED | introduced | The fixture claims to follow the measured endpoint and says a flushed context that is never closed gets no final message. Probe P1 got `isFinal` 20012 ms after the last audio. | Nothing in practice: the binding always sends the close with the text. |
| F4 | `adapters/inference-elevenlabs/tests/adversary_u2_no_audio_context.rs:4` | INFEASIBLE | introduced | The case's server sends no audio for `…`, and its comment says punctuation yields no audio. Probe P4 got one audio message (2973 B) for `…`. The guard is still useful for other no-audio cases; only the premise in the comment is wrong. | Not the hosted endpoint, as measured. The coordinator's correction asked for this server behaviour. |
| F5 | `examples/realtime-voice/tests/topology_latency.rs:604` | CONFIRMED (by reading, not run) | introduced | The only checks on the new "tts offer to audio" column cannot fail: one is `!is_empty()`, the other compares the value with the same vector it was read from, and so does the row check at :682. Two mutants would pass: dropping the once-per-segment check in `compare.rs` (`heard.insert`), or reading a constant duration. Also, under the fake clock this column is wall-clock time while the other columns are on the fake clock. | Acceptance item "in the latency report". The brief does not let me add tests in realtime-voice. |
| F6 | `examples/realtime-voice/examples/compare.rs:391` (also :513 and :518) | CONFIRMED | pre-existing (same lines at 016e083d) | The ElevenLabs cascade TTS uses `Endpoint::new()`, i.e. the global host. The project's only key is EU-residency and the global host refuses it, so the "tts offer to audio" column can never be measured for ElevenLabs. Fix: `Endpoint::from_env()`. | `compare --cascade-voice elevenlabs` with the EU key. |
| F7 | `adapters/inference-elevenlabs/src/config.rs:135` | CONFIRMED | introduced | Setting `ELEVENLABS_RESIDENCY` to an empty value is refused. `inference-openai` and `inference-deepslate` `from_env` treat an empty value as unset. This is documented and tested on purpose, but it is inconsistent across adapters. | A `.env` template with `ELEVENLABS_RESIDENCY=` left empty. |

**5. What I attacked and could not break**
- **Cancel mid-generation:** the cancelled context's audio keeps arriving interleaved with the next segment. The binding drops it, it does not move the next segment's read deadline, and its final clears it from the cancelled set.
- **Reopen after a cancel:** only messages of the segment being spoken mark it as heard. So a connection that dies after stale cancelled messages still reopens once, and the cancelled set is cleared on reopen.
- **Segment completion and order:** the completion event comes after the segment's last frame, and a segment ending on half a sample is refused.
- **`SynthesisOutput::first_audio` bookkeeping:** the per-segment lists only grow in `offer`, `move_segment` keeps each segment's position, and a segment discarded before any frame has none.
- **Message size:** `MAX_MESSAGE_SIZE` is 4 MiB. The probe's largest chunk (57678 B, 7.2 s of μ-law) would be about 850 KB as base64 in `pcm_44100`, well under it.
- **Fixture against the log:** a close mid-generation cuts no audio and the final follows the last audio; a close after quiet gets the final at once. Both match.
- **Residency names and hosts:** they match the saved protocol summary, and `with_residency` keeps the configured timeouts.

**6. Paths written outside the worktree**

All under `<scratch>/wave-p1/U2/review-2/`:
- `run-case.sh`, `case-residency.log`
- `mutant.sh`, `mutant.diff`, `mutant.log`
- `mutant-tree/` (22 MB copy of the source)
- `mutant-target/` (484 MB build directory; safe to delete)
- `suite.sh`, `suite.log`

**7. Findings block**

```findings
- file: adapters/inference-elevenlabs/src/config.rs
  line: 124
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Endpoint::from_env is called by no non-live case, so reading the wrong variable passes the suite; guarded by adversary2_u2_residency_env.rs
- file: adapters/inference-elevenlabs/src/lib.rs
  line: 37
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: crate doc and CHANGELOG:34 say 99% (22.5 of 22.7 s) came after the close; the probe log gives 22158 ms after it, 97.7%
- file: adapters/inference-elevenlabs/tests/support/mod.rs
  line: 17
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: fixture claims the measured model yet says an unclosed flushed context gets no final; probe P1 got one after 20012 ms
- file: adapters/inference-elevenlabs/tests/adversary_u2_no_audio_context.rs
  line: 4
  category: contract-drift
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the case models punctuation-only text as yielding no audio, while probe P4 got one audio message for the same ellipsis
- file: examples/realtime-voice/tests/topology_latency.rs
  line: 604
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the tts offer to audio column is checked only by non-emptiness and against its own source vector, so a wrong value or a duplicate entry passes
- file: examples/realtime-voice/examples/compare.rs
  line: 391
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: pre-existing
  message: the ElevenLabs cascade arm uses Endpoint::new, so the project's EU-residency key is refused and the new column cannot be measured for ElevenLabs
- file: adapters/inference-elevenlabs/src/config.rs
  line: 135
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: an empty ELEVENLABS_RESIDENCY is refused while the other adapters' from_env treat an empty value as unset
```