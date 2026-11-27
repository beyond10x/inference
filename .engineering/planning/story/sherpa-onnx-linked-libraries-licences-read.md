---
format: aep.planning-md/3
id: story:sherpa-onnx-linked-libraries-licences-read
kind: story
status: implemented
title: The licence of every library the sherpa-onnx archive links is read and recorded
tags:
- from-finding
relations:
- informed_by: review-result:local-tts-adapter-security-1
- serves: vision:inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T03:08:46Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-25T03:08:46Z", actor: "human:operator", revision: 4}
- {from: "active", to: "implemented", at: "2026-11-27T15:57:25Z", actor: "human:operator", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
## Context

The security review of the local TTS adapter (review-result:local-tts-adapter-security-1, finding 9)
found that `CODE_LICENSES` of `adapters/inference-sherpa-onnx-tts` names 4 components, while the
build script of `sherpa-onnx-sys` lists 13 libraries the native archive links (`build.rs:16-30`),
among them `ucd`, `kissfft-float`, `kaldi-decoder-core` and `ssentencepiece_core`. The four entries
cover five of the thirteen (`sherpa-onnx-c-api` and `sherpa-onnx-core` are both sherpa-onnx). Nobody
read the licences of the other eight. The repository's licence policy asks that a local adapter's
backend code is Apache-2.0, MIT or more permissive, with one recorded exception. The STT adapter
`adapters/inference-sherpa-onnx` links the same upstream build without its TTS parts, so the gap
predates the TTS adapter.

## Planned scope

- Read the licence file of each library the pinned sherpa-onnx archive links, at the version the
  pin builds, for both sherpa-onnx adapters.
- Record one code-licence entry per library in each adapter, with the source of the reading.
- A library whose licence the policy does not admit goes to the owner as a decision before any
  release that follows.

## Acceptance

- Each adapter's code-licence list has one entry per library its native archive links, and a test
  compares the list with the library list of the pinned `sherpa-onnx-sys` build script (state
  transition: the pin changes and the test goes red until the list follows).
