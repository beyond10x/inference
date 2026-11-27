---
format: aep.planning-md/3
id: approval-record:linked-libraries-licences-admitted
kind: approval-record
status: draft
title: The libraries the sherpa-onnx archives link are admitted under their licences (2026-11-27)
relations:
- decides: story:sherpa-onnx-linked-libraries-licences-read
revision: 1
---
## Decision

The owner's decision of 2026-11-27 (D23 C), taken under the owner's standing order that open
decisions are taken on the recommended option: the libraries the licence reading of 2026-11-25
found outside the licence rule of `AGENTS.md` are admitted, only as built into the upstream
sherpa-onnx and onnxruntime archives that the user's build fetches, never committed or
distributed here, each recorded as a code-licence entry of its own:

- Eigen (MPL-2.0), in both archives and so for both sherpa-onnx adapters;
- ucd-tools (GPL-3.0-or-later), read as part of the espeak-ng exception of
  `approval-record:sherpa-tts-gpl-phonemiser`, for `inference-sherpa-onnx-tts` alone;
- the GCC runtime (GPL-3.0-or-later WITH GCC-exception-3.1) and the static pieces of glibc
  (LGPL-2.1-or-later), under their runtime and linking exceptions.

Every other licence of backend code still needs the owner's word.

## Consequence

The local text-to-speech adapter (`inference-sherpa-onnx-tts`) merges with its licence lists, and
the release 0.34.0 follows it.
