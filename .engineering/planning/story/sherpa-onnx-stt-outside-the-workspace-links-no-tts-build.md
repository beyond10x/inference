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
revision: 1
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
base of the wave. The two sentences were corrected in the wave to state the condition; the build
is unchanged.

## Planned scope

- A build of `inference-sherpa-onnx` outside the workspace does not link the archive with
  text-to-speech unless the caller asks for it: the adapter's own build step refuses to go on
  without a checked archive, or fetches and checks the pinned archive without TTS itself.
- The guide for a consumer names the one setting or step this needs.
- The design is chosen before the change: a build script in the adapter, a feature of
  `sherpa-onnx-sys`, or a documented required variable with a compile error naming it.

## Acceptance

- A crate outside the workspace that depends on `inference-sherpa-onnx` by path or Git and sets
  nothing either fails to build with an error naming the setting, or links the archive whose
  sha256 is the pinned one (state transition: the dependency is built for the first time).
- No build of the speech-recognition adapter downloads an archive without checking its hash.
