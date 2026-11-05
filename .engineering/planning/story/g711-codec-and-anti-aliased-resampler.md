---
format: aep.planning-md/3
id: story:g711-codec-and-anti-aliased-resampler
kind: story
status: active
title: G.711 mu-law and A-law and an anti-aliased resampler in inference-audio
summary: Bridge a hosted voice model to an 8 kHz phone leg inside the library.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T13:07:47Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-05T13:07:47Z", actor: "human:timo", revision: 3}
---
## Context

A downstream adopter wants voice agents for phone calls. A phone line carries G.711 audio at
8 kHz, μ-law or A-law. `inference-audio` names both encodings (`PcmMulaw`, `PcmAlaw`) but carries
no codec and no resampler. The device adapter resamples linearly with no anti-aliasing filter
(limitations page, "Audio"). A hosted model that speaks 16 or 24 kHz PCM therefore cannot be
bridged to a phone leg inside the library.

## Planned scope

- `crates/inference-audio`:
  - G.711 μ-law and A-law encode and decode to and from `PcmS16le`, by ITU-T G.711's tables;
  - a band-limited resampler between 8, 16, 24 and 48 kHz with an anti-aliasing low-pass filter.
- Pure Rust, no new native dependency.

## Acceptance

- μ-law and A-law round-trip every 16-bit sample within G.711's quantisation step, and decode
  every one of the 256 code words to the table's value.
- Resampling a sine above the target band's Nyquist frequency down leaves it attenuated by a
  stated figure, where the linear resampler folds it back.
- A 24 kHz to 8 kHz μ-law chain on a speech fixture stays within a stated error of a reference
  computed in the test.
