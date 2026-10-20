---
format: aep.planning-md/3
id: epic:local-tts-adapter
kind: epic
status: draft
title: Local TTS adapter, standalone
summary: One self-hosted text-to-speech adapter usable with inference-tts alone; synthesised, queued and played kept apart.
relations:
- derived_from: architecture-design:inference
- depends_on: epic:contracts-and-testkit
- serves: vision:inference
revision: 3
---
## Context

Step 3 of the first demonstrator: "run one self-hosted TTS adapter as a standalone library"
(`docs/design/inference.md:1173`). The TTS contract is §7.3 (lines 378-390): declare whether input
arrives incrementally or as a complete utterance (385) and whether audio is emitted incrementally
(386); distinguish synthesised, queued and played, and only the sink confirms playback (388);
alignment is optional (389); an uncommitted token stream is never safe to speak by default (390).

The backend and hardware are undecided: `decision-blocker:first-adapters`,
`decision-blocker:reference-hardware`, `decision-blocker:weight-licenses`. The review recommends
sherpa-onnx (`docs/design/review-2026-10-18.md`, F6).

## Outcome

One self-hosted TTS adapter crate under `adapters/`, usable with `inference-tts` alone, that
consumes committed segments, emits audio chunks, and reports synthesised, queued and played apart.

## Scope

- One adapter crate implementing the `inference-tts` contract over the chosen backend.
- Its capability record (§17.2): voices, languages, formats, `incremental_input`, `output_streaming`,
  `audio_text_alignment` as observed.
- Mapping to `inference.stream`: `QueueForSynthesis` on a committed segment, `AcknowledgePlayback`
  from the sink, `DiscardSegment` on cancellation.

## Exit

- §23.1 test 3 (line 1107): input completion and audio emission semantics reflected in the record.
- `inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis` and
  `inference.stream.OutputSegment/transition/play/by/inference.stream.AcknowledgePlayback/played`
  pass with this adapter and a sink in the loop.
- §23.2 lines 1114-1117 against this adapter; audio timestamps and format transformations valid
  (1117).

## Out of scope

A second TTS backend (backend-replacement); speculative synthesis gated on commitment (§11.3 line
627, local-voice-cascade); hosted TTS.

## Decisions

Settled on 2026-10-20 (owner took the defaults):

- Backends (`decision-blocker:first-adapters`): sherpa-onnx for STT, TTS and VAD; llama.cpp
  `llama-server` over its local HTTP API for the LLM. The swap is STT, sherpa-onnx to whisper.cpp.
- Hardware (`decision-blocker:reference-hardware`): i9-10900K, 62 GiB, RTX 3090 24 GiB, Linux
  x86_64; tests run on CPU, the GPU is optional.
- Licences (`decision-blocker:weight-licenses`): code Apache-2.0 or MIT, weights CC BY 4.0 or more
  permissive, each checkpoint recorded at adoption; tests read weight paths from environment
  variables and are ignored without them.
