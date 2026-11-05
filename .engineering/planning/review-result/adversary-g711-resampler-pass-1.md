---
format: aep.planning-md/3
id: review-result:adversary-g711-resampler-pass-1
kind: review-result
status: active
title: 'Adversary pass on the G.711 codec and the resampler: two doc drifts, no defect'
summary: No red case in 14; two documentation notes fixed in the one correction
relations:
- reviews: story:g711-codec-and-anti-aliased-resampler
revision: 1
---
## Pass

This is the adversary pass on the G.711 codec and the resampler, at `3d9dc7c`. The cases are in
`1463575`: `adversary1_g711_tables.rs` and `adversary1_resampler_spectrum.rs`, test files only.

verdict: nothing found
cases: executed 40→54, red 0
origin: introduced 3 / pre-existing 0 / undecided 0

## Cases

- **G.711 tables** (`adversary1_g711_tables.rs`, 7 passed):
  - The μ-law and A-law tables are rebuilt from the segment starts, steps and decision values of
    G.711 Tables 1a and 2a. They share no helper or constant with the code under test.
  - All 256 code words decode to the table values. Every `i16` encodes into its decision
    interval, with either side allowed on an exact tie.
  - The silence words hold: 0xFF in μ-law and 0xD5 in A-law.
  - `to_encoding` was run over all 16 encoding pairs, at 1, 2, 3 and 5 channels and at 0, 1 and
    3 frames. Format, position and length are right.
  - Every `pcm_f32le` pair is refused `Unsupported`. Every data length mismatch is refused
    `DataLength`, and nothing panics.
- **Resampler spectrum** (`adversary1_resampler_spectrum.rs`, 7 passed):
  - **Frame splits:** seeded random splits, frames of length 0 and 1 and frames shorter than the
    filter included, give the one-shot output exactly. After each call the output holds
    `ceil(n*L/M)` samples. This holds for all 16 rate pairs.
  - **Delay:** `delay` matches the measured lag on every pair.
  - **Stop band:** content at 0.9, 0.98, 1.02 and 1.1 of the lower Nyquist frequency is 84.4 to
    95.6 dB down. A tone exactly at Nyquist is 80.5 dB down.
  - **Pass band:** at 0.8 of Nyquist the error is within −0.0009 dB.
  - **Full-scale input:** a full-scale signal clamps without wrapping.
  - **Frame positions:** they stay within one sample of input position × ratio over 400 random
    frames, with no drift. A restart or an overlap is refused as `Discontinuous`.
- **Suite:** `inference-audio --lib --tests` gives 54 passed. Clippy and fmt are clean.

## Routing

The two confirmed notes are documentation drift. They are fixed in `744ebb7`, the unit's one
correction:

- `to_encoding` no longer says that every conversion to the same encoding is a copy;
- the resampler doc now says that `resample_frame` allocates the data of each frame it returns.

The μ-law to A-law note has no verdict. The adversary recalled G.711's direct conversion tables
from memory and did not check them. The conversion goes through the 16-bit sample, and the doc
now says it does not use those tables. It is recorded and not changed.

```findings
[{"file":"crates/inference-audio/src/g711.rs","line":187,"category":"judgement","severity":"note","origin":"introduced","message":"mu-law to A-law conversion goes through the 16-bit sample rather than G.711's direct conversion tables; whether the two differ for some code words was not checked against the tables"},{"file":"crates/inference-audio/src/g711.rs","line":148,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"to_encoding doc says the same encoding returns a copy, but pcm_f32le to pcm_f32le is refused as Unsupported"},{"file":"crates/inference-audio/src/resample.rs","line":33,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"Resampler doc says processing allocates only to grow its buffers, but resample_frame allocates a new Vec for each returned frame"}]
```
