---
format: aep.planning-md/3
id: story:deepgram-streaming-stt-adapter
kind: story
status: draft
title: Deepgram streaming STT as a hosted STT adapter
summary: A hosted STT layer for a phone-audio cascade.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 1
---
## Context

For a cascade on phone audio, a downstream adopter's survey names Deepgram as the strongest
speech-to-text layer: it is strong on 8 kHz audio, accepts native μ-law, has an EU endpoint, and
its Flux model detects turns itself. inference's STT adapters are local only: sherpa-onnx
(streaming, English) and whisper.cpp (batch).

## Planned scope

- A streaming STT binding of the `inference-stt` contract over Deepgram's live transcription
  WebSocket, in `adapters/inference-deepgram`:
  - partial and final transcripts;
  - the provider's end-of-turn signal, where one is given;
  - its claims record;
  - the protocol from public documentation, saved with its date.

## Acceptance

- The backend-neutral STT checks pass against a local fixture server.
- The live test is ignored and behind feature `live`. It needs a Deepgram API key with STT
  access.
