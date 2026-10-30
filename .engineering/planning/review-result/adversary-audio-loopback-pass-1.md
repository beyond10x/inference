---
format: aep.planning-md/3
id: review-result:adversary-audio-loopback-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the audio-device loopback: 4 introduced boundary findings'
relations:
- reviews: story:audio-device-adapter
revision: 1
---
## Report

Adversary pass 1 on `impl/audio-loopback` at `a5732a3` (adversary tests committed on top as `3599497`,
`adapters/inference-audio-device/tests/adversary1_loopback.rs`, 14 cases).

```
unit: U2 story:audio-device-adapter, impl/audio-loopback at a5732a3 (adversary tests committed on top as 3599497)
verdict: INFEASIBLE
cases: executed 34→48, red 4
origin: introduced 4 / pre-existing 0 / undecided 0
needs-coordinator: decide whether finding 4 is a defect or the intended design; choose the fix for findings 1–2
```

Suite run: `cargo test --no-fail-fast -p inference-audio-device --all-features --test loopback --test hardware --test capture --test playout --test adversary1_loopback --lib`:
lib 2 ok, adversary1_loopback FAILED 10 passed 4 failed, capture 13 ok, hardware 1 passed 1 ignored, loopback 6 ok, playout 12 ok; exit 101.

None of the four is reached by the example's settings (0.5 s delay against a 2 s playout buffer,
16 kHz in 20 ms frames); `run` has no caller but the example and tests.

Attacked and not broken: frame limits 0 and 1, a zero delay, a delay that is not a whole number of
frames, a microphone that fails or overruns mid-run, devices at native rates other than the
requested one, `played_out` with refused frames, both devices closed on every way out. The
fake-host twin of the hardware flow calls the same function with the same assertions.

```findings
- file: adapters/inference-audio-device/src/loopback.rs
  line: 177
  category: boundary
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: the delay phase checks neither the stop flag nor the run, so with a delay longer than the playout buffer a pre-set stop still blocks about one second per refused delay frame (5.01 s, adversary1_loopback.rs:137)
- file: adapters/inference-audio-device/src/loopback.rs
  line: 170
  category: boundary
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: a delay longer than the playout buffer is accepted and the delay phase blocks on the speaker without reading the microphone, so the capture ring overflows (9600 samples dropped, adversary1_loopback.rs:191)
- file: adapters/inference-audio-device/src/loopback.rs
  line: 166
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the delay is counted in frame durations while each frame holds rounded-down samples, so 1 s at 44.1 kHz in 1 ms frames queues 44000 samples, more than one frame short (adversary1_loopback.rs:223)
- file: adapters/inference-audio-device/src/loopback.rs
  line: 214
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a refused captured frame still advances the renumbering clock, so report.played says 960 where the speaker played 640 samples (adversary1_loopback.rs:392)
```
