---
format: aep.planning-md/3
id: story:local-tts-adapter-under-licence-policy
kind: story
status: draft
title: A local TTS adapter whose backend fits the licence policy, spoken by the offline cascade
relations:
- decomposes: epic:local-tts-adapter
revision: 1
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
