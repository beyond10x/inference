---
format: aep.planning-md/3
id: review-result:security-elevenlabs-tts-cancel-pass-1
kind: review-result
status: active
title: Security pass 1 on the ElevenLabs TTS provider-side cancel
relations:
- reviews: story:elevenlabs-tts-provider-cancel
revision: 2
---
unit: story:elevenlabs-tts-provider-cancel, `016e083d..7f1c4237` plus two untracked test files, worktree impl-elevenlabs-tts-provider-cancel
verdict: NEEDS-CHANGE
cases: executed 203→206, red 3
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 5 paths
needs-coordinator: acceptance item 1 (the provider receives a context close on cancel) was deliberately dropped because of the billed probe, and the story's title, summary and acceptance still promise it. Only the coordinator can rewrite the story or accept the gap.

**1. Diff stat**

`git --no-pager diff --stat` is empty: no tracked file changed. `git status --short` shows only these two new files, both tests:
```
?? adapters/inference-elevenlabs/tests/security_u2_changelog_breaking.rs
?? adapters/inference-elevenlabs/tests/security_u2_residency_echo.rs
```
Nothing was staged or committed. No implementation file was touched.

**2. Cases added, each run alone before the suite**

- `security_u2_residency_echo.rs`. The crate forbids `unsafe`, so a test cannot set the variable in its own process. Each case therefore re-runs the ignored child case `child_prints_the_refusal_of_from_env` in a child process with `ELEVENLABS_RESIDENCY` set.
  - `a_key_in_the_residency_variable_is_not_repeated_by_the_refusal`: with a key-shaped value, neither the Display nor the Debug text of the error may contain it. **Red.**
  - `the_refusal_of_the_residency_variable_stays_one_line`: a value containing a line break must not put a line break into the error's Display text. **Red.**
- `security_u2_changelog_breaking.rs`
  - `the_new_measure_variant_and_measured_field_of_realtime_voice_are_listed_as_breaking`: the Unreleased `### Breaking` list must name the `realtime-voice` `Measure` change. **Red.**

Output of the cases alone, verbatim:
```
cargo test -p inference-elevenlabs --test security_u2_residency_echo
test child_prints_the_refusal_of_from_env ... ignored, a child of the parent cases of this file, run with ELEVENLABS_RESIDENCY set
test the_refusal_of_the_residency_variable_stays_one_line ... FAILED
test a_key_in_the_residency_variable_is_not_repeated_by_the_refusal ... FAILED
---- the_refusal_of_the_residency_variable_stays_one_line stdout ----
panicked at adapters/inference-elevenlabs/tests/security_u2_residency_echo.rs:93:5:
the refusal's Display carries a line break from the value: "ELEVENLABS_RESIDENCY is `eu\nERROR forged line`; it takes global, us, eu, in or sg, or is unset for global"
---- a_key_in_the_residency_variable_is_not_repeated_by_the_refusal stdout ----
panicked at adapters/inference-elevenlabs/tests/security_u2_residency_echo.rs:77:5:
the refusal's Display repeats the value of ELEVENLABS_RESIDENCY: ELEVENLABS_RESIDENCY is `fixture-key-d41d8cd98f00b204`; it takes global, us, eu, in or sg, or is unset for global
test result: FAILED. 0 passed; 2 failed; 1 ignored; ...
exit 101

cargo test -p inference-elevenlabs --test security_u2_changelog_breaking
test the_new_measure_variant_and_measured_field_of_realtime_voice_are_listed_as_breaking ... FAILED
(prints the Unreleased Breaking list: duplex, pipeline, facade, elevenlabs ConfigError only)
test result: FAILED. 0 passed; 1 failed; ...
exit 101
```

**3. Suite run, after the cases existed**

- Command: `cargo test -p inference-elevenlabs --features keychain,duplex,stt -p inference-tts --no-fail-fast`. Feature `live` was not enabled.
- Result: 203 passed, 3 failed, 1 ignored (my child case). Exit 101.
- Only two targets failed: `security_u2_changelog_breaking` and `security_u2_residency_echo`. Every existing target is green.
- The `before` count of 203 is this same run with my two test binaries taken out (206 − 3). I did not do a separate earlier run.

**4. Findings, covering `7f1c4237` plus the untracked tests**

1. **The residency error repeats the variable's value as-is.** `adapters/inference-elevenlabs/src/error.rs:68` (variant added at `:36`, value passed in at `src/config.rs:137`). NEEDS-CHANGE, introduced.
   - Measured: both cases above. A key placed in `ELEVENLABS_RESIDENCY` appears in the error's Display and Debug text, and a line break in the value forges a second log line.
   - What reaches it: the documented error path, meaning any value that is not a residency name. That error is then printed by `examples/live.rs:87` (via `Problem::Missing(error.to_string())`) and by the panics at `tests/stt_live.rs:42` and `tests/tts_cancel_live.rs:83`.
   - Nothing in the repository puts a key into this variable. The leak needs an operator to put the key in the wrong one of two neighbouring variables.
   - The crate already avoids this elsewhere: `InvalidVoiceId` carries no value.
   - Fix I would suggest: drop the value, or print it only when it is short and `{value:?}`-escaped.
2. **The CHANGELOG is missing two breaking changes in `realtime-voice`.** `CHANGELOG.md:22` (Unreleased Breaking list). Changed code is at `examples/realtime-voice/src/compare.rs:337` and `:1841`/`:1848`. NEEDS-CHANGE, introduced.
   - `Measure` gains `TtsOfferToAudio`, so `Measure::ALL` becomes `[Self; 8]`. `Measured` gains the public field `tts_segment_first_audio`. Neither type is `#[non_exhaustive]`.
   - These are listed only under Added. The same Unreleased Breaking list already records a change to another unpublished example (`local-voice-cascade`'s `CascadeIds`), and AGENTS.md asks for a Breaking section for public type changes.
3. **The story's acceptance is not met as written.** `adapters/inference-elevenlabs/tests/cancel.rs:23`. INFEASIBLE, introduced.
   - The test asserts that a cancel sends nothing. Acceptance item 1 requires that the fixture server receives the context close on cancel.
   - The billed probe log shows a close does not stop generation, so the requirement cannot be met against the hosted endpoint. The story text needs updating, which is the coordinator's call.
4. **A comment contradicts the probe.** `adapters/inference-elevenlabs/tests/adversary_u2_no_audio_context.rs:4`. CONFIRMED, introduced, note.
   - The comment says a punctuation-only text yields no audio, and its server sends none for `…`.
   - The probe log (P4) shows `…` returned one audio message of 2973 B.
   - The test still covers a real defensive path; only the claim in the comment is wrong.

**5. Reviewed and could not fault**

- `ELEVENLABS_RESIDENCY` maps only to the five documented hosts:
  - `Residency::named` matches exactly, and anything not valid Unicode is refused.
  - `from_env` is the only place in `src/` that reads the environment for a URL.
  - `request_url` still enforces `wss://` (or `ws://` to loopback only) and refuses a query or fragment.
  - The voice id cannot change the host, because the path starts with `/`.
- `tts_cancel_live.rs` handles the key safely:
  - The key goes only in the `xi-api-key` header, marked `set_sensitive`, and the credential is dropped right after.
  - Its panics show only `CredentialError`, the header error (no value, `InvalidHeaderValue { _priv: () }`) and tungstenite errors.
  - The printed URL is cut at `?` and never contains the key.
  - The probe log contains no `sk_`.
- Frames of a discarded segment are dropped:
  - `src/synthesiser.rs` is unchanged since the base.
  - The discard removes queued events and marks the context as cancelled, and the step loop drops every later message of that context.
  - The new paced test checks this while the server is still generating.
- Bounds still hold:
  - `src/connection.rs` is unchanged since the base, with `MAX_MESSAGE_SIZE` at 4 MiB (`:26`) applied to messages and frames.
  - The probe applies the same limit and puts a timeout on every read.
- The fixture server in `tests/support/mod.rs` matches the probe log for P1, P2 and P3: audio is never cut by a close, the final message comes only after a close, and a flush alone never gets one within a test. It simply does not model the 20 s inactivity timeout.
- `ConfigError::Residency` is listed under Breaking. Moving `Residency` keeps `agent::Residency` working through a re-export.
- No private names, hosts or persons in the committed diff. `story:` ids already appear in committed tests at the base.

**6. Paths written outside the worktree**

- `<scratch>/wave-p1/U2/review-2/security-case.log`
- `<scratch>/wave-p1/U2/review-2/security-changelog-case.log`
- `<scratch>/wave-p1/U2/review-2/suite.sh`
- `<scratch>/wave-p1/U2/review-2/suite.log`
- The directory itself, `<scratch>/wave-p1/U2/review-2/`, which I created.

The build output went to the worktree's own `target/`. My worktree session lease (`secrev-u2-p1-f7875192`) is released.

**7. Findings block**

```findings
- file: adapters/inference-elevenlabs/src/error.rs
  line: 68
  category: integrity
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: ConfigError::Residency repeats the raw ELEVENLABS_RESIDENCY value in Display and Debug, so a key set there is printed by the live example and live-test panics, and a line break in it forges a log line
- file: CHANGELOG.md
  line: 22
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the new Measure::TtsOfferToAudio variant (Measure::ALL now [Self; 8]) and public Measured field of realtime-voice are listed only under Added, not under Breaking
- file: adapters/inference-elevenlabs/tests/cancel.rs
  line: 23
  category: acceptance
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: acceptance item 1 requires the fixture server to receive a context close on cancel; the test asserts a cancel sends nothing, because the billed probe showed a close does not stop generation, so the story text needs changing
- file: adapters/inference-elevenlabs/tests/adversary_u2_no_audio_context.rs
  line: 4
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the comment and server model say a punctuation-only text yields no audio, but the probe log shows the ellipsis returned one 2973 B audio message
```