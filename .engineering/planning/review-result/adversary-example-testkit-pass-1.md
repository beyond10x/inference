---
format: aep.planning-md/3
id: review-result:adversary-example-testkit-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the example and testkit findings
relations:
- reviews: story:example-and-testkit-findings
revision: 1
---
unit: RF-examples, story:example-and-testkit-findings; covers unit head bfdbffe plus adversary commit a217240 (branch impl/example-and-testkit-findings)
verdict: NEEDS-CHANGE
cases: executed 80→82, red 2
origin: introduced 0 / pre-existing 0 / undecided 2
needs-coordinator: yes. The N35 red case falls under this story's acceptance but its path is older code (`end_turn`). Decide whether it is fixed in this unit or moved out.

**1. Diff stat (bfdbffe..a217240).** Test files only:
`adapters/inference-sherpa-onnx/tests/adversary_rf1_stream_rate_floor.rs | 20`, `crates/inference-testkit/tests/adversary_rf1_mock_call_during_output.rs | 106`. Committed as `a217240` by `b10x-bot[bot]` (author and committer).

**2. Cases added.** Each was run alone first, and both were red on that first run:
- `cargo-run.sh test -p inference-testkit --features duplex --test adversary_rf1_mock_call_during_output`: 0 passed, 1 failed, rc=101.
  - `a_call_of_an_unoffered_tool_scripted_for_input_during_output_fails_the_session` panics with "a Reply::Call of the unoffered tool `thermometer` scripted for input during output passes the run without a failure".
  - The events show the second turn transcribed ("and the temperature"), then OutputAudio ×2, ResponseUsageRecorded and ResponseCompleted. There is no SpeechToolCallProposed and no SpeechSessionFailed.
  - Cause: `mock.rs:461-474` (`end_turn`, response in progress, non-restarting model) splices only `Reply::Say` into the response and drops every other reply without a word.
- `cargo-run.sh test -p inference-sherpa-onnx --features stt --test adversary_rf1_stream_rate_floor`: 0 passed, 1 failed, rc=101.
  - `a_rate_of_one_hertz_is_refused_as_a_streams_rate` fails with `left: Ok(1) right: Err(UnsupportedRate { rate: 1 })`.

**3. Suite run, after the cases existed:**
- `test -p inference-testkit --features duplex --lib --test adversary_rf1_mock_call_during_output`: lib 59 ok; adversary 0/1, rc=101.
- `test -p inference-sherpa-onnx --features stt --lib --test adversary_rf1_stream_rate_floor`: lib 8 ok; adversary 0/1, rc=101.
- `test -p repo-podcast --test start_turn_failure --test show`: show 12 ok, start_turn_failure 1 ok, rc=0.
- Clippy `-D warnings` on both added targets: rc=0. rustfmt check and `scan-text` are clean.

**4. Findings:**
- **N35 (warning, NEEDS-CHANGE, undecided), `crates/inference-testkit/src/duplex/mock.rs:461`**
  - What was measured: the red case above. Acceptance "a mis-scripted Reply::Call naming an unoffered tool fails the mock run" does not hold when the turn ends during output.
  - What reaches it: a script that puts a Call on an input-during-output turn, on the native mock with `auto_interrupt` off. The existing `input_during_output_is_answered…` test has that shape. Offered tools are dropped the same way.
  - Named fix: in `end_turn`, fail the session (or append the Call as steps) when the reply is not a `Say`.
  - Origin: no base tree, so not run at base. Reading the code, base has no path that would fail this session.
- **N32 (note, INFEASIBLE, undecided), `adapters/inference-sherpa-onnx/src/stt.rs:150`**
  - What was measured: the red case above. `stream_rate` has no floor. A 1 Hz first frame becomes the stream's rate.
  - The later-frame path (`:812-818`) builds `Resampler::new(frame_rate, stream_rate)` without consulting `stream_rate` at all. A 48 000-sample frame at 1 Hz into a 384 kHz stream yields about 1.8e10 f32. The planned scope said "outside the supported set".
  - What reaches it: nothing found. Frames come from the pipeline; constructed, as the review did for N32.
  - Named fix: a lower bound in `stream_rate`, and a check of every frame's rate before `Resampler::new`.
- **N32 test (note, CONFIRMED, introduced), `adapters/inference-sherpa-onnx/src/stt/tests.rs:172`**
  - `a_first_frame_at_an_absurd_rate_gives_the_stream_the_models_rate` never runs `advance()`. It asserts `stream_rate`, a constant pin (`MODEL_RATE == 16_000`) and the `Resampler` separately.
  - Mutating `:806-808` back to `get_or_insert(frame_rate)` stays green. Only an ignored checkpoint test could catch it, so no runnable failing case.

**5. Attacked and could not break:**
- N34: refused `create_response` closes the session, reads the close, Billed is emitted, and no other early return exists between open and `self.live`.
- N35 base path: a refused step fails the session. `ledger.fail` cannot be refused there, because `require_open` is checked first.
- N29 no-op holds: GPT-Live requires `auto_interrupt` on (`live_duplex.rs:18`), so `barge_in` cuts current and playing. `talk_over::gpt_live_streaming_on` (3 cases, from 9fee977) is present. Not re-run.
- N39 no-op holds: `AGENTS.md:86` says 0.20.0, and `Cargo.toml:14-15` pins 0.20.0.
- `MAX_STREAM_RATE` is exact at the bound (`tail_padding(384_000)` ok, `384_001` refused). The new public items are additive.

```findings
[
  {"file": "crates/inference-testkit/src/duplex/mock.rs", "line": 461, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "undecided", "message": "end_turn drops a non-Say reply for a turn ending during output, so a Reply::Call of an unoffered tool passes the run; red: adversary_rf1_mock_call_during_output::a_call_of_an_unoffered_tool_scripted_for_input_during_output_fails_the_session"},
  {"file": "adapters/inference-sherpa-onnx/src/stt.rs", "line": 150, "category": "boundary", "severity": "note", "verdict": "INFEASIBLE", "origin": "undecided", "message": "stream_rate has no lower bound and later frames bypass it, so a 1 Hz frame is upsampled up to 384000x before anything refuses it; constructed, no caller found; red: adversary_rf1_stream_rate_floor::a_rate_of_one_hertz_is_refused_as_a_streams_rate"},
  {"file": "adapters/inference-sherpa-onnx/src/stt/tests.rs", "line": 172, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "a_first_frame_at_an_absurd_rate_gives_the_stream_the_models_rate never runs advance(), so reverting the get_or_insert_with wiring stays green; no runnable failing case without the checkpoint"}
]
```
