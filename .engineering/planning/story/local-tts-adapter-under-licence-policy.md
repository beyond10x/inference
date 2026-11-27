---
format: aep.planning-md/3
id: story:local-tts-adapter-under-licence-policy
kind: story
status: active
title: A local TTS adapter whose backend fits the licence policy, spoken by the offline cascade
relations:
- decomposes: epic:local-tts-adapter
- serves: vision:inference
scope:
- confidence: inferred
  path: .cargo/config.toml
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: Cargo.lock
- confidence: inferred
  path: Taskfile.yml
- confidence: inferred
  path: adapters/inference-sherpa-onnx
- confidence: inferred
  path: adapters/inference-sherpa-onnx-tts
- confidence: cited
  path: adapters/inference-sherpa-onnx/src/lib.rs
- confidence: inferred
  path: crates/inference-docs/src/adapters.rs
- confidence: inferred
  path: crates/inference-docs/src/status.rs
- confidence: cited
  path: examples/local-voice-cascade
- confidence: cited
  path: ux/scenarios/find-limitations/scenario.json
- confidence: inferred
  path: website/data/status.json
- confidence: inferred
  path: website/docs/examples/local-voice-cascade.md
- confidence: inferred
  path: website/docs/getting-started.md
- confidence: inferred
  path: website/docs/guides/run-the-local-voice-cascade-offline.md
- confidence: inferred
  path: website/docs/reference/adapters.md
- confidence: cited
  path: website/docs/status/limitations.md
- confidence: inferred
  path: website/product.json
revision: 24
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:08:07Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-24T13:08:07Z", actor: "human:operator", revision: 4}
---
## Context

The offline voice cascade binds the mock synthesiser: the answer is printed and the speaker plays
10 ms of silence per word (`website/docs/status/limitations.md:14-20`, main at c0ba0028). A spoken
local agent therefore cannot run without a hosted TTS. epic:local-tts-adapter is the plan for one
self-hosted adapter.

The backend is decided (corrected 2026-11-24; this section said before that it "waits on the
licence policy"). sherpa-onnx's TTS build links espeak-ng (GPL-3.0), and the licence rule admits
Apache-2.0, MIT or more permissive backend code. The owner admitted espeak-ng on 2026-10-22 as a
named exception, only inside the sherpa-onnx native library and its `espeak-ng-data`, with the
adapter as its own opt-in crate (approval-record:sherpa-tts-gpl-phonemiser).

## Planned scope

- Pick a TTS backend whose code and weights fit the licence policy, recorded with its licence and
  the date read.
- One adapter crate under `adapters/` implementing the `inference-tts` contract over it, with its
  capability record, held by `inference_testkit::tts::conformance`.
- The local voice cascade speaks through it in place of the mock.

## Acceptance

- `inference_testkit::tts::conformance` passes against the adapter.
- The offline cascade example speaks a reply through the adapter with no hosted call.
- `limitations.md` no longer lists "No local text-to-speech".

## Re-read 2026-11-24

Re-read on 2026-11-24 for the wave of approval-record:wave-2026-11-24-dub-voice.

- The backend is decided. approval-record:sherpa-tts-gpl-phonemiser records the owner's choice of
  2026-10-22: espeak-ng (GPL-3.0) is admitted as a named exception, only as part of the sherpa-onnx
  native library and its `espeak-ng-data`, and the local TTS adapter is its own opt-in crate, never
  a dependency of a contract crate. decision-blocker:sherpa-tts-gpl-phonemiser is cleared. The
  Context above and `website/docs/status/limitations.md:14-20` say the backend "waits on the
  licence policy"; both were written after that decision and do not cite it. This is the
  coordinator's reading of the store, not a new decision: the first bullet of the planned scope is
  answered by sherpa-onnx offline TTS, which runs VITS voices in the Piper format.
- A consumer met this gap first. A screencast dub on 2026-11-24 needed a local voice, could not use
  this repository and ran a local synthesiser binary directly. Its need: a voice directory as
  configuration, text in, audio out, behind the same contract as the hosted synthesisers.
- A synthesiser run as an external process is not what the exception names, so this story does not
  take that route.
- The weights: the adapter pins one checkpoint for its record and its ignored tests, CC BY 4.0 or
  more permissive, with its licence, source URL and hash in the capability record. A caller may
  point the adapter at another voice directory; the licence of that voice is the caller's.
- Open for the implementor, found on 2026-11-24: `sherpa-onnx-sys` declares `links = "sherpa-onnx"`,
  so one workspace build holds one native library, and the STT adapter links the archive built
  without TTS (`website/docs/status/limitations.md:56`). The unit states how the two archives are
  chosen and what an STT-only build links afterwards.

## Scope

Read by a story-scoper on 2026-11-24 at `b3333a4a1`; nothing was built or run.

- **Primary surface:** a new adapter crate under `adapters/`, a workspace member by the glob in
  `Cargo.toml:3` — cited
- **Crate:** `adapters/inference-sherpa-onnx-tts` — inferred
- **Files:** `examples/local-voice-cascade/src/local.rs:68,351-363,501,527`,
  `examples/local-voice-cascade/Cargo.toml:12` — cited, the mock binding the story replaces
- **Documents:** `website/docs/status/limitations.md:14-20` — cited, named by the acceptance
- **Also:** `ux/scenarios/find-limitations/scenario.json:7` — cited, its truth text is the heading
  the acceptance removes
- **Also likely:** `crates/inference-docs/src/adapters.rs:76-222` (the bindings list),
  `crates/inference-docs/src/status.rs:347-352` (the planned entry), the generated
  `website/docs/reference/adapters.md` and `website/data/status.json` — inferred
- **Also likely:** `Taskfile.yml:76-92` (the task that fetches the native archive),
  `.cargo/config.toml:2`, `adapters/inference-sherpa-onnx/src/lib.rs:11-13` and
  `tests/licence.rs:67` — inferred, only where the choice of archive changes them
- **Also likely:** `AGENTS.md:86-90` (the licence rule, which does not carry the exception of
  approval-record:sherpa-tts-gpl-phonemiser yet), `website/product.json:35`,
  `website/docs/examples/local-voice-cascade.md:38`,
  `website/docs/guides/run-the-local-voice-cascade-offline.md:25`,
  `website/docs/getting-started.md:51` — inferred
- **Held for the coordinator:** `CHANGELOG.md` and `website/docs/reference/environment.md` (another
  unit of the wave edits a sentence there); the unit hands the text over
- **Symbols:** `inference_tts::Synthesiser`, `inference_tts::SynthesisHandle`,
  `inference_testkit::tts::conformance`, `sherpa_onnx::OfflineTts` — cited
- **ESS:** no new noun. The adapter is an instance of `inference.model.Adapter`
  (`ess/domains/model.yaml:198`), `execution_form` `native_ffi`, `locality` `same_process`; its
  checkpoint is an `inference.model.Model` with `code_license`, `weight_license`, `checksum` — cited
- **Confidence:** medium

Facts the plan rests on:

- `sherpa-onnx-sys` 1.13.8 declares `links = "sherpa-onnx"`: one cargo invocation runs its build
  script once, with one `SHERPA_ONNX_LIB_DIR`. Two crates may depend on it; both keep feature
  `shared`.
- The workspace pins the archive built without TTS (`Taskfile.yml:79`, `.cargo/config.toml:2`).
  That library exports the `SherpaOnnxOfflineTts*` C symbols, so a crate calling
  `sherpa_onnx::OfflineTts` links against it; creating a synthesiser on it then fails at run time
  (read from the library's own string, not run).
- The archive built with TTS for the same release is `sherpa-onnx-v1.13.8-linux-x64-shared-lib`,
  sha256 `3892d184be41027e18165e67f549cd4e4cdd8dcd73ac5579e97afd55e14e30b6` (release API, read
  2026-11-24).
- The unignored test `no_gpl_phonemiser_is_linked`
  (`adapters/inference-sherpa-onnx/tests/licence.rs:67`) fails when the workspace archive is the
  one with TTS.

Not established: whether a run-time library path wins over the path the build script emits; the
sample rate of a Piper-format voice against the resampler's set of 8, 16, 24 and 48 kHz
(`crates/inference-audio/src/resample.rs:59`); whether the generation callback hands over the new
samples or all samples so far; the licence of any Piper-format voice (licensed per voice; the
voice the consumer used has not been read and is recorded as unknown).
