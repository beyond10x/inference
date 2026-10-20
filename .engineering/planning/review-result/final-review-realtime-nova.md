---
format: aep.planning-md/3
id: review-result:final-review-realtime-nova
kind: review-result
status: active
title: Final adversary review, realtime-nova area
relations:
- reviews: story:nova-sonic-adapter
- reviews: story:native-speech-contract
- reviews: story:audio-device-adapter
- reviews: story:realtime-voice-demo
revision: 1
---
```
unit: realtime-nova area (story:nova-sonic-adapter, story:native-speech-contract, story:audio-device-adapter, story:realtime-voice-demo), working tree at wave/18 c733f12 plus 4 untracked test files
verdict: NEEDS-CHANGE
cases: executed 169→175, red 3
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 12 paths (logs in <host path> mutant copy already removed)
needs-coordinator: a live Nova check, to show whether findings 1 and 3 happen with the real provider
```

**1. Diff stat**

`git --no-pager diff --stat` prints nothing: no tracked file changed. The `git status` lines below are all test files:
```
?? adapters/inference-audio-device/tests/adversary2_g711.rs
?? adapters/inference-aws/tests/adversary2_nova_boundaries.rs
?? crates/inference-duplex/tests/adversary2_probe.rs
?? examples/realtime-voice/tests/adversary2_nova_barge_in.rs
```

**2. Cases added (each run alone first, before any suite run)**

| File | Asserts | Now |
|---|---|---|
| `crates/inference-duplex/tests/adversary2_probe.rs` | A model that answers the opening and never reads `during` gets no `native_duplex` evidence | red |
| `adapters/inference-aws/tests/adversary2_nova_boundaries.rs` | Nova (replay) silent after a tool result fails the session as `stalled` | red |
| `examples/realtime-voice/tests/adversary2_nova_barge_in.rs` | Speech over a Nova answer that is finished but still playing stops playout | red |
| `adapters/inference-audio-device/tests/adversary2_g711.rs` | G.711 encode/decode match hand-worked reference bytes at 0, ±full scale, ±1000 | green (guard) |

Red output, verbatim:
```
...the probe called that native duplex: Ok(NativeDuplexEvidence { response_id: ResponseId(0b8d7e1d-...), before: "", after: "The capital of France is Paris." })
three seconds after the tool result, ten bounds of 300 ms, the session is Open and nothing failed it; events after the result: [SpeechToolCallAnswered(..), ResponseCompleted(..)]
the operator spoke over the answer and playout went on, 720 samples still queued: []
```

Mutant check: I removed `.min(32_635)` at `convert.rs:84` in a scratch copy. The crate's existing suite stayed green (all its binaries `ok`). `adversary2_g711` went red with `left: [255, 255, 127, 206, 78]` against `right: [255, 128, 0, 206, 78]`: under the mutant, full scale +1.0 encodes as silence. I ran `cargo clean` on the copy and removed it.

**3. Suite runs (after the cases existed)**

`cargo test -p <crate> --all-features --no-fail-fast`:

| Crate | Exit | Passed | Failed | Ignored | Failing case |
|---|---|---|---|---|---|
| inference-duplex | 101 | 28 | 1 | – | `adversary2_probe` |
| inference-aws | 101 | 72 | 1 | 3 | `adversary2_nova_boundaries` |
| inference-audio-device | 0 | 62 | 0 | 1 | – |
| realtime-voice | 101 | 10 | 1 | – | `adversary2_nova_barge_in` |

- The "before" count of 169 is these after-runs minus my 6 cases. Each of my files is its own test binary, so this equals deselecting them.
- `cargo clippy --all-targets --all-features -D warnings` exited 0 for all four crates.
- `cargo fmt --check` exited 0 for all four, after I ran rustfmt on my own G.711 file.

**4. Findings**

| # | file:line | Verdict / origin | What was measured | What reaches it |
|---|---|---|---|---|
| 1 | `crates/inference-duplex/src/conformance.rs:168` | NEEDS-CHANGE / introduced | `before` holds only the one event that ended the first loop. An `OutputAudio` leaves it empty, so a transcript already recorded counts as "after". The probe also never checks that the opening is free of the marker. | `nova_sonic_duplex_live.rs:205-215` sends the same clip as opening and `during` ("What is the capital of France?", marker "france"). If Nova does not restart, the probe returns false evidence; I don't know if it does. Fix: drain every event already recorded into `before` before sending `during`, and refuse a probe whose opening transcript says the marker. |
| 2 | `adapters/inference-aws/src/duplex.rs:790` | NEEDS-CHANGE / introduced | Answering a call does not start the liveness bound. Nothing is awaited after a tool result, so a silent provider leaves the session open with no failure, against `ess/domains/duplex.yaml:1019` ("never with silence"). | The demo's time tool on Nova (`nova_config`, idle limit 30 s). Fix: when `!capabilities.manual_response`, `SessionLedger::answer` arms the bound the way a commit does. |
| 3 | `examples/realtime-voice/src/lib.rs:385` | NEEDS-CHANGE / introduced | The demo stops playout only on `SpeechStarted` or `ResponseCancelled`. The Nova binding never emits `SpeechStarted`, and drops a cancel of a completed response (`duplex.rs:533-542`). So an answer that has finished generating keeps playing over the user. | Acceptance: "interrupts it mid-answer and hears it stop". Happens whenever generation outruns playback; the speaker queues up to 4 s (`lib.rs:139-146`). Whether that is the common live case: I don't know. Fix: the binding emits `speech_started` on Nova's interruption notice and on the user transcript start, or the demo stops playout on `InputCommitted` for models with no speech observations. |
| 4 | `crates/inference-testkit/src/duplex/mock.rs:484` | CONFIRMED / introduced | The `server_turns` mock, which stands in for Nova, emits `SpeechStarted`; the Nova binding emits none. `wiring.rs` `a_provider_side_barge_in_stops_playout_without_truncating` passes on an event the binding never delivers. | Every demo test run on `nova_config`. |
| 5 | `adapters/inference-audio-device/src/convert.rs:84` | CONFIRMED / introduced | Dropping the mu-law clip survives the crate's suite. The round-trip unit test never feeds a value above 32 124. | Any `pcm_mulaw` capture at full scale. Guarded now by `adversary2_g711`. |
| 6 | `adapters/inference-aws/src/duplex.rs:566` | INFEASIBLE / introduced | Judgement, no case. Response boundaries are inferred from block order. A stray assistant block arriving after a commit takes the "open" slot, and the real answer then maps to `turns.last` and is dropped. A user transcript before the interruption notice completes the response instead of cancelling it. A mid-turn `END_TURN` would complete it early. | Depends on the order Nova sends events, which I don't know; the replay sends the notice first and one segment per reply. |

**5. Attacked and not broken**
- G.711 decode matches the classic reference coder for all 256 bytes; encode matches at 0, ±32767 and ±1000. The VAD's copy of the decoder is identical.
- ESS enums against Rust enums (turn detection, sensitivity, failure kind, states, encodings): names and order agree.
- Ledger close and fail cascades, and response and call id reuse: the pass-1 cases hold, and I found nothing new by reading.
- Usage summing per response against the replay: the deltas add up to the documented totals.
- OpenAI-path barge-in (truncate at the played position, queued responses cut at 0): the wiring cases hold, and reading the code found no stale-audio replay.
- Runtime handling: `Pool::run` is only called from the demo's own thread, and dropping the runtime does not block.

**6. Paths written outside the worktree**

All 12 are in `<host path>`:
- `test-{inference-duplex,inference-aws,inference-audio-device,realtime-voice}.log`
- `clippy-{same four}.log`
- `fmt-{same four}.log`

The mutant copy (`mutant-g711/`) was cleaned and removed.

**7. Findings block**

```findings
- file: crates/inference-duplex/src/conformance.rs
  line: 168
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: native_duplex_evidence gives evidence to a model that never reads the input sent during output, because before holds only the first output event and the opening may already say the marker, as the Nova live probe's does.
- file: adapters/inference-aws/src/duplex.rs
  line: 790
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: after a tool result on a model that creates every response, nothing arms the liveness bound, so a provider that never answers leaves the session open in silence instead of failing it as stalled.
- file: examples/realtime-voice/src/lib.rs
  line: 385
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: with the Nova binding, which emits no SpeechStarted and drops interruption of a completed response, speech over an answer still playing never stops playout.
- file: crates/inference-testkit/src/duplex/mock.rs
  line: 484
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the server_turns mock that stands in for Nova emits SpeechStarted, which the Nova binding never does, so the demo's Nova barge-in case passes on an event the real binding cannot deliver.
- file: adapters/inference-audio-device/src/convert.rs
  line: 84
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: removing the mu-law clip at 32635 makes full scale encode as silence and the crate's suite stays green; adversary2_g711 now catches it.
- file: adapters/inference-aws/src/duplex.rs
  line: 566
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: response boundaries are inferred from block order, so an assistant block arriving after a commit or a user transcript before the interruption notice would misattribute or drop the next answer, depending on an event order of Nova's that was not observed.
```