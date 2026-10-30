---
format: aep.planning-md/3
id: story:audio-device-adapter
kind: story
status: active
title: Microphone capture and speaker playout as an adapter
summary: 'adapters/inference-audio-device over cpal: a mic source and a speaker sink with played position and stop, tested without hardware.'
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: cited
  path: Cargo.toml
- confidence: cited
  path: adapters/inference-audio-device
- confidence: cited
  path: adapters/inference-audio-device/src/loopback.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-19T21:13:04Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-19T21:13:04Z", actor: "human:timo", revision: 4}
---
## Context

The operator asked on 2026-10-19 for local microphone capture and playout to a headset or speaker
from Rust, as an adapter, so a voice pipeline can be tried on a laptop against the realtime models.
The design keeps sources and sinks as ordinary nodes with typed ports (§9, sources and sinks) and
audio as bounded binary frames on realtime paths (`docs/design/inference.md:324`); a device is an
adapter like any model (`adapters/`, lines 203-213).

## Planned scope

- `adapters/inference-audio-device` (crate `inference-audio-device`) over `cpal` (ALSA,
  PulseAudio or PipeWire on Linux, CoreAudio, WASAPI), with no change to any contract crate.
- Device listing with the default input and output marked; selection by name or default.
- Capture: a microphone source producing `inference-audio` frames in a requested format (mono PCM16
  at 8, 16 or 24 kHz, 20 ms frames by default), resampled and down-mixed from the device's native
  format; bounded buffering that counts and reports dropped frames instead of blocking the device
  callback.
- Playout: a speaker sink taking `inference-audio` frames, resampled to the device; it reports the
  played position (samples actually handed to the device), can be stopped at once (clear unplayed
  audio, for barge-in), and reports underruns.
- Both as pipeline nodes (a source and a sink implementing the node trait of
  `inference-pipeline`) and as plain handles usable without the runtime.
- Tests that need no audio hardware: format conversion and resampling, frame slicing, the bounded
  buffer and its drop count, stop-and-clear, played-position accounting, against a fake device
  behind the same internal trait the cpal backend implements. A hardware test, `#[ignore]` and
  behind a `device` feature, records two seconds from the default input and plays a tone and the
  recording back.
- An example, `mic-loopback`, that plays the microphone back to the speaker with a fixed delay, so
  the operator can check the devices by ear.

## Acceptance

`cargo test -p inference-audio-device --all-features` passes on a machine with no audio device;
`cargo run -p inference-audio-device --example mic-loopback` on a laptop plays the microphone back
through the default output; the ignored hardware test passes on a laptop with a microphone.

## Out of scope

Echo cancellation and noise suppression (the operator's headset or the provider handles them);
the realtime voice demo that wires the device to a native speech model (`story:realtime-voice-demo`).

## Re-check 2026-10-29

Re-checked against `origin/main` at `de88601` on 2026-10-29 for the wave that moved `ess` to 0.76.0,
and left out of that wave.

- Built on `main`: `adapters/inference-audio-device` (capture, playout with played position and stop,
  device listing, source and sink nodes, the fake device), the `mic-loopback` example and the
  ignored hardware test `tests/hardware.rs` behind feature `device`.
- The part of the acceptance a machine without audio hardware can show passes in that wave's full
  gate (`task check`, `cargo test --workspace --all-features`, which includes
  `cargo test -p inference-audio-device --all-features`).
- Open: the two device parts of the acceptance, `mic-loopback` heard on a laptop and the ignored
  hardware test run with a microphone. Both need the operator at a laptop with a microphone and a
  speaker; no code is planned for them. The story stays active until that run is recorded here.

## Covered without a device 2026-10-30

Covered without a device on 2026-10-30 (approval-record:wave-0-9-0), in release 0.9.0:

- `inference_audio_device::loopback::run` holds what `mic-loopback` does, generic over `AudioHost`;
  the example calls it on the cpal host. A delay or a frame the playout queue cannot hold is refused
  before any device opens; the stop flag and the run are checked during the delay; the delay is
  counted in samples, short by less than one frame; the microphone is closed when forwarding ends,
  before the speaker drains.
- The hardware test's flow (record, positions, level, tone, stop position, playback, no stream
  error) runs on the fake host on every test run (`the_hardware_flow_passes_on_a_fake_laptop`);
  the device run stays ignored.
- Two adversary passes (review-result:adversary-audio-loopback-pass-1, 4 findings;
  review-result:adversary-audio-loopback-pass-2, 2 new, 0 carried; ledger compared by hand because
  `aep plan artifact findings` paired pass 2 with an older review of this story). All six fixed.

Open: the device parts of the acceptance. They are the operator's run on a laptop with a microphone
and a speaker:

```
cargo run -p inference-audio-device --example mic-loopback --features device
cargo test -p inference-audio-device --features device --test hardware -- --ignored --nocapture
```
