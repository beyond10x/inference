---
format: aep.planning-md/3
id: review-result:adversary-audio-loopback-pass-2
kind: review-result
status: active
title: 'Adversary pass 2 on the audio-device loopback: drain drops counted, dead delay-refusal surface'
relations:
- reviews: story:audio-device-adapter
revision: 1
---
## Report

Adversary pass 2 on `impl/audio-loopback` at `eca73ba` (the correction to pass 1); adversary tests
committed on top as `9b6e38a`, `adapters/inference-audio-device/tests/adversary2_loopback.rs`, 9 cases.

```
unit: U2 story:audio-device-adapter pass 2, covering eca73ba plus test commit 9b6e38a on impl/audio-loopback
verdict: NEEDS-CHANGE
cases: executed 52→61, red 1
origin: introduced 2 / pre-existing 0 / undecided 0
needs-coordinator: whether the drain finding holds U2 or goes to its own story
```

Suite run: `cargo test --no-fail-fast -p inference-audio-device --all-features --test loopback --test hardware --test capture --test playout --test adversary1_loopback --test adversary2_loopback --lib`:
lib 2 ok; adversary1_loopback 15 ok; adversary2_loopback 8 passed 1 FAILED; capture 13 ok; hardware 1 ok 1 ignored; loopback 9 ok; playout 12 ok; exit 101.

Red case: `a_one_second_delay_on_the_default_buffers_reports_no_audio_dropped_after_forwarding`:
the report says the microphone dropped 8320 device frames, all after forwarding ended (left 8320, right 0), the same in three runs.

Attacked and not broken: the refusal boundaries (delay equal to the buffer, a delay that fills the
queue exactly, a frame equal to the queue and one sample over, a zero buffer), the stop and run
checks between delay frames, the `played` arithmetic with several refused frames and with a refused
frame after the played position, the 100 ms push wait in real time. Pass 1's rewritten case 2 lost
its `dropped_device_frames == 0` assertion; pass 2's real-time case restores it while forwarding.

```findings
- file: adapters/inference-audio-device/src/loopback.rs
  line: 270
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the microphone stays open and unread through the drain, so with any delay longer than the capture ring the report (which the example prints) counts device frames dropped after forwarding ended as lost audio, 8320 for a 1 s delay on default buffers
- file: adapters/inference-audio-device/src/loopback.rs
  line: 126
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: after the up-front refusal no built-in host can refuse a delay frame, so delay_refused, Refused { delay true } and the example's matching branch are dead public surface
```
