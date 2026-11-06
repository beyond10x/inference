---
format: aep.planning-md/3
id: story:openai-speech-tts-adapter
kind: story
status: implemented
title: OpenAI streamed speech as a hosted TTS adapter
summary: A second hosted TTS binding, for consumers already on OpenAI speech.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T17:26:37Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-06T17:26:38Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-06T18:22:07Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Context

A downstream adopter speaks fixed flow prompts through OpenAI's speech endpoint
(`POST /v1/audio/speech`, streamed) with its own client, resampling 24 kHz PCM to 8 kHz μ-law
itself, because inference has no OpenAI TTS binding. inference's only hosted TTS adapter is
ElevenLabs (story:elevenlabs-streaming-tts-adapter). The adopter wants to move its speech calls
onto inference.

## Planned scope

- A binding of the `inference-tts` contract over OpenAI's streamed speech endpoint, in
  `adapters/inference-openai` behind a feature:
  - model and voice configurable, the text spoken as given;
  - PCM output resampled to the requested rate and, where asked, encoded to G.711 with
    `inference-audio`'s codec and anti-aliased resampler;
  - cancel mid-utterance stops the stream and still yields the usage known so far;
  - the request's character limit enforced with a typed error;
  - its claims record, held equal to the code, and the protocol from public documentation saved
    with its date.

## Acceptance

- The backend-neutral TTS checks pass against a local fixture server.
- A fixture run asking for μ-law 8 kHz receives μ-law 8 kHz frames.
- A cancel mid-utterance yields no further frame and one usage record.
- The live test is ignored and behind feature `live`. It needs an OpenAI API key.
