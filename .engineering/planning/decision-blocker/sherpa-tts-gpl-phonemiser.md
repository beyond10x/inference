---
format: aep.planning-md/3
id: decision-blocker:sherpa-tts-gpl-phonemiser
kind: decision-blocker
status: cleared
title: Nobody has decided whether the GPL-3.0 espeak-ng inside sherpa-onnx TTS is admitted under the code-licence policy
summary: sherpa-onnx TTS links espeak-ng (GPL-3.0); the policy admits Apache-2.0, MIT or BSD code only; the owner picks admit, change backend, or patch the build.
relations:
- blocks: epic:local-tts-adapter
revision: 2
transitions:
- {from: "open", to: "cleared", at: "2026-10-21T22:53:46Z", actor: "human:timo", revision: 2}
---
## Question

The owner chose sherpa-onnx offline TTS (`decision-blocker:first-adapters`) and admitted backend
code under Apache-2.0 or MIT, BSD counting as more permissive (`decision-blocker:weight-licenses`,
`AGENTS.md:59-63`). sherpa-onnx itself is Apache-2.0, but its TTS build links espeak-ng, which is
GPL-3.0. Read on 2026-10-20:

- sherpa-onnx v1.13.8 `CMakeLists.txt:71` turns `SHERPA_ONNX_ENABLE_TTS` on by default, and lines
  569-573 include `espeak-ng-for-piper` and `piper-phonemize` whenever it is on. No option builds
  TTS without them.
- `cmake/espeak-ng-for-piper.cmake:4` fetches a fork of espeak-ng; GitHub reports that fork as
  GPL-3.0. Upstream espeak-ng is GPL-3.0-or-later.
- The published Rust binding `sherpa-onnx` 1.13.8 (Apache-2.0) depends on `sherpa-onnx-sys`
  1.13.8, whose default `static` mode links a prebuilt library downloaded from the sherpa-onnx
  GitHub releases, `espeak-ng` among its static libraries (`build.rs:16-30`).
- The phoneme-based voices sherpa-onnx packages (Kokoro, Piper VITS) ship `espeak-ng-data`, data
  of the same GPL-3.0 project, read at run time.

Piper, the swap backend named for TTS, phonemises through `piper-phonemize`, the library linked
beside espeak-ng above; verify at adoption. The two decisions conflict for TTS, and nothing in the
store says which wins.

## What it stops

Every story of `epic:local-tts-adapter` that links sherpa-onnx: the adapter crate, its pinned
checkpoint and its checkpoint-gated test. Backend-neutral work is not stopped:
`story:tts-adapter-conformance` is drafted.

It also touches `epic:local-stt-adapter`: `sherpa-onnx-sys` declares `links = "sherpa-onnx"`, so
one workspace build holds one native sherpa-onnx library for STT and TTS together, and the prebuilt
one links espeak-ng whether or not TTS is used. Whether the STT adapter can link a source build with
TTS off (`SHERPA_ONNX_LIB_DIR`) is a technical check for that epic, not blocked here; with a TTS
adapter in the same workspace it cannot.

## Options

| option | what follows | cost |
|---|---|---|
| A. admit espeak-ng (GPL-3.0) as a named exception for the sherpa-onnx native library and its `espeak-ng-data` | the TTS adapter is its own opt-in crate under `adapters/`, never a dependency of a contract crate; its capability record and README carry espeak-ng as a separate code-licence entry | any binary linking the adapter is a GPL-3.0 combined work; this repository ships no binary, but its tests link it |
| B. keep the policy strict and change the first TTS backend to one without espeak-ng | reopens the TTS row of `decision-blocker:first-adapters`; one candidate is Kyutai TTS (`delayed-streams-modeling`, GitHub reports Apache-2.0; weights `kyutai/tts-1.6b-en_fr`, CC BY 4.0 on its model card) | its runtime form on the CPU baseline of `decision-blocker:reference-hardware` is unverified (I do not know it); its voice embeddings are licensed separately |
| C. sherpa-onnx TTS with a lexicon-only model over a build patched to drop espeak-ng | backend decision kept, policy kept | a maintained fork of the sherpa-onnx build; no upstream option exists (`CMakeLists.txt:569`) |

## Candidate checkpoint under option A

Kokoro-82M: weights Apache-2.0 per its model card,
https://huggingface.co/hexgrad/Kokoro-82M (revision f3ff3571791e39611d31c381e3a41a3af07b4987, read
2026-10-20), packaged by sherpa-onnx as `kokoro-en-v0_19.tar.bz2` under
https://github.com/k2-fsa/sherpa-onnx/releases/tag/tts-models. At adoption: the archive hash, the
licence of its voices file, and the espeak-ng-data it carries.

## Recommendation

Option A: it keeps both decisions where they bind most. The backend stays sherpa-onnx, the code this
repository commits stays Apache-2.0, nothing GPL is committed or distributed here, and the exception
is visible in the capability record. Reversible: the adapter is one opt-in crate.

## Default on silence

None. A licence outside the policy needs the owner's word (`AGENTS.md:61`), so the sherpa-onnx
TTS adapter is not drafted until this clears.

## Clears when

The owner picks an option and it is written into this body; for A, also as the named exception in
the licence rule of `AGENTS.md`; for B, also into `decision-blocker:first-adapters`.
