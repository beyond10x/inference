---
format: aep.planning-md/3
id: story:sherpa-onnx-stt-outside-the-workspace-links-no-tts-build
kind: story
status: draft
title: A build of the sherpa-onnx recogniser outside the workspace links no archive with TTS unasked
tags:
- from-finding
relations:
- informed_by: review-result:local-tts-adapter-security-1
- serves: vision:inference
revision: 4
---
## Context

The security review of the local TTS adapter (review-result:local-tts-adapter-security-1, finding 7)
found that the claim "no GPL-licensed phonemiser is linked" of the speech-recognition adapter
`adapters/inference-sherpa-onnx` holds only for a build inside this repository's workspace. There
`.cargo/config.toml` points `SHERPA_ONNX_LIB_DIR` at the archive without text-to-speech that the
`sherpa-onnx-archive` task fetched and checked against its pinned sha256. A consumer who depends
on the adapter through Git, as `website/docs/getting-started.md` describes, has no such setting:
the build script of `sherpa-onnx-sys` then downloads its default archive, which is the build with
text-to-speech and espeak-ng (GPL-3.0), and checks no hash (`build.rs:129`, `:392`, `:199-209` at
the pinned version; read by the reviewer, not built). The text and the pin are the same at the
base of the wave.

The wave of 2026-11-24 did not correct the two sentences that carry the claim: the head comment of
`adapters/inference-sherpa-onnx/src/lib.rs` (lines 11-13) and the bullet in
`website/docs/status/limitations.md` (lines 48-49). A permission check refused the edit in the
implementor's session, and the operator decided that the wave ships without it. Both sentences
still state the claim without its condition. `SHERPA_ONNX_ARCHIVE_DIR` cannot select the archive
without text-to-speech, because the build script looks there for the default archive's name; the
setting that does is `SHERPA_ONNX_LIB_DIR` alone.

## Planned scope

- A build of `inference-sherpa-onnx` outside the workspace does not link the archive with
  text-to-speech unless the caller asks for it: the adapter's own build step refuses to go on
  without a checked archive, or fetches and checks the pinned archive without TTS itself.
- The guide for a consumer names the one setting or step this needs.
- The design is chosen before the change: a build script in the adapter, a feature of
  `sherpa-onnx-sys`, or a documented required variable with a compile error naming it.
- The two sentences that carry the claim say what holds after the change: the head comment of
  `adapters/inference-sherpa-onnx/src/lib.rs` and the bullet on sherpa-onnx speech recognition in
  `website/docs/status/limitations.md`. Until the build is changed they state the condition (a
  build inside this workspace), the archive a build outside it downloads, and the setting
  `SHERPA_ONNX_LIB_DIR` with the archive's name and sha256.

## Acceptance

- A crate outside the workspace that depends on `inference-sherpa-onnx` by path or Git and sets
  nothing either fails to build with an error naming the setting, or links the archive whose
  sha256 is the pinned one (state transition: the dependency is built for the first time).
- No build of the speech-recognition adapter downloads an archive without checking its hash.
- Neither sentence says "no GPL-licensed phonemiser is linked" without the condition under which
  it holds (state transition: the two sentences are changed).
