---
format: aep.planning-md/3
id: approval-record:sherpa-tts-gpl-phonemiser
kind: approval-record
status: draft
title: espeak-ng GPL-3.0 admitted as a named exception for sherpa-onnx TTS; the adapter is an opt-in crate
relations:
- decides: decision-blocker:sherpa-tts-gpl-phonemiser
revision: 1
---
## Which stop

`decision-blocker:sherpa-tts-gpl-phonemiser`: whether the GPL-3.0 espeak-ng that sherpa-onnx TTS
links, and the `espeak-ng-data` its phoneme voices read, are admitted under the code-licence policy
that otherwise admits Apache-2.0, MIT or BSD code only.

## Reason

`owner-decided`: the owner answered on 2026-10-22, quoted verbatim: "33+34 as you recommend". The
recommendation was option A.

## What was decided

Option A:

- espeak-ng (GPL-3.0) is admitted as a named exception, only as part of the sherpa-onnx native
  library and its `espeak-ng-data`.
- The local TTS adapter is its own opt-in crate. It is never a dependency of a contract crate.

The other options, a different TTS backend or a patched sherpa-onnx build without espeak-ng, were
not taken.
