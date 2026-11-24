---
format: aep.planning-md/3
id: story:local-tts-adapter-under-licence-policy
kind: story
status: active
title: A local TTS adapter whose backend fits the licence policy, spoken by the offline cascade
relations:
- decomposes: epic:local-tts-adapter
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T13:08:07Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-24T13:08:07Z", actor: "human:timo", revision: 4}
---
## Context

The offline voice cascade binds the mock synthesiser: the answer is printed and the speaker plays
10 ms of silence per word (`website/docs/status/limitations.md:14-20`, main at c0ba0028). A spoken
local agent therefore cannot run without a hosted TTS. epic:local-tts-adapter is the plan for one
self-hosted adapter; it has no story yet. The backend waits on the licence policy: sherpa-onnx's
TTS build links espeak-ng (GPL-3.0), and the policy admits Apache-2.0, MIT or more permissive
backend code.

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
