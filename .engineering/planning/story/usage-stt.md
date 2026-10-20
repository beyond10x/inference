---
format: aep.planning-md/3
id: story:usage-stt
kind: story
status: active
title: 'Speech recognition usage: input audio and local compute per recognition, mock, sherpa-onnx and whisper.cpp'
summary: The STT contract measures input audio per recognition; local adapters add measured compute; the STT node sends the record on its usage port.
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
- depends_on: story:usage-llm
scope:
- confidence: cited
  path: adapters/inference-sherpa-onnx/src/stt.rs
- confidence: cited
  path: adapters/inference-whispercpp/src/stt.rs
- confidence: cited
  path: crates/inference-stt
- confidence: cited
  path: crates/inference-testkit/src/stt
- confidence: cited
  path: crates/inference-testkit/src/stt.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T19:17:05Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-20T19:17:05Z", actor: "human:timo", revision: 4}
---
## Context

A recognition (`Recogniser::start`, one `RecognitionHandle`) is one model call of the STT contract
(`crates/inference-stt/src/lib.rs:1-20`, 636-661). No recogniser reports usage today: neither the
mock (`crates/inference-testkit/src/stt.rs:318`) nor the local adapters over sherpa-onnx
(`adapters/inference-sherpa-onnx/src/stt.rs:545`) and whisper.cpp
(`adapters/inference-whispercpp/src/stt.rs:496`), and neither backend reports a quantity of its own.
What there is to count is measured here: the audio taken and the time spent in the backend.

## Planned scope

- `inference-stt`: `TranscriptEvent::Usage(UsageRecorded)`, the last event of a recognition once
  input has ended and its last span is committed. `TranscriptOutput`, the bookkeeping every
  implementation shares, measures `input_audio_ms` from the frames offered (samples over sample
  rate, `measured`) and `requests` 1, so every recogniser reports both without code of its own; an
  implementation adds `local_compute_ms` through one method, `unknown` when it adds none. A
  recognition dropped before input ended yields no record (no call completed); this is stated in
  the crate documentation.
- sherpa-onnx and whisper.cpp: `local_compute_ms` measured around their decode calls, `measured`.
  The STT mock adds none, so its record shows `unknown` compute.
- `inference-testkit`: the STT node (`src/stt/node.rs`) sends the record on its `usage` output port
  under the port convention of `story:usage-llm`, filling run, node and session.
- The weighted adapter tests, ignored without their checkpoints, assert the record of a known clip:
  `input_audio_ms` equal to the clip length and `local_compute_ms` above 0.

## Domain relations

- UsageRecord to Run, Node and Session, each zero or one, and to Adapter and Model, each one:
  `ess/domains/usage.yaml`, `inference.usage.UsageRecord`, relations `run`, `node`, `session`,
  `adapter`, `model`, as `story:usage-domain` declares them. The adapter is the recogniser adapter
  record (inferred from `crates/inference-stt/src/lib.rs:4-5`: a `Recogniser` carries the one
  adapter record it was declared with).

## Acceptance

A recognition of 1.5 s of audio on the STT mock ends with one `TranscriptEvent::Usage` whose
`input_audio_ms` is 1500 `measured`, `requests` 1 and `local_compute_ms` `unknown`, and a pipeline run
with the STT node delivers it on the exposed `usage` egress with run and node set.

- With checkpoints present, the sherpa-onnx and whisper.cpp tests show `input_audio_ms` equal to the
  clip length and `local_compute_ms` measured; without them they are listed as ignored.
- `task check` passes.
