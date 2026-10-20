---
format: aep.planning-md/3
id: story:usage-tts
kind: story
status: draft
title: 'Speech synthesis usage: characters, output audio and compute per synthesis, the mock and the backend-neutral TTS checks'
summary: The TTS contract measures characters and output audio per utterance; the TTS checks every adapter runs require one usage record.
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
- depends_on: story:usage-stt
- depends_on: story:tts-adapter-conformance
revision: 1
---
## Context

A synthesis (`Synthesiser::start`, one `SynthesisHandle`, one utterance) is one model call of the
TTS contract (`crates/inference-tts/src/lib.rs:893-923`). The only synthesiser today is the testkit
mock (`crates/inference-testkit/src/tts.rs:217`); a sherpa-onnx synthesiser waits on
`decision-blocker:sherpa-tts-gpl-phonemiser`. Future adapters are covered by putting the measuring
in the contract bookkeeping and the check in the backend-neutral TTS checks of
`story:tts-adapter-conformance` (`crates/inference-testkit/src/tts/conformance.rs`), which every TTS
adapter runs.

## Planned scope

- `inference-tts`: `SynthesisEvent::Usage(UsageRecorded)`, the last event of a synthesis once the
  utterance has ended and its last segment is synthesized. `SynthesisOutput` measures
  `input_characters` (Unicode scalar values of the segment text taken), `output_audio_ms` (samples of
  the audio emitted over sample rate) and `requests` 1, each `measured`; an implementation adds
  `local_compute_ms` through one method, `unknown` when it adds none. A synthesis dropped before
  its utterance ended yields no record; segments discarded after synthesis still count, since the
  audio was generated.
- `inference-testkit`: the mock adds no compute; the TTS node (`src/tts/node.rs`) sends the record
  on its `usage` output port under the port convention of `story:usage-llm`; the backend-neutral
  TTS checks gain one check that a completed utterance yields exactly one usage record whose
  characters and audio milliseconds match what was offered and emitted.

## Domain relations

- UsageRecord to Run, Node and Session, each zero or one, and to Adapter and Model, each one:
  `ess/domains/usage.yaml`, `inference.usage.UsageRecord`, relations `run`, `node`, `session`,
  `adapter`, `model`, as `story:usage-domain` declares them. The adapter is the synthesiser adapter
  record (inferred from `crates/inference-tts/src/lib.rs:897-899`, `Synthesiser::adapter`).

## Acceptance

An utterance of two committed segments of 12 and 30 characters on the TTS mock ends with one
`SynthesisEvent::Usage` whose `input_characters` is 42 and whose `output_audio_ms` equals the audio
the mock emitted, both `measured`, and the backend-neutral TTS checks pass on the mock with the new
usage check.

- A pipeline run with the TTS node delivers the record on its exposed `usage` egress with run and
  node set.
- `task check` passes.
