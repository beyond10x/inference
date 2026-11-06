---
format: aep.planning-md/3
id: story:elevenlabs-streaming-tts-adapter
kind: story
status: implemented
title: ElevenLabs streaming TTS as a hosted TTS adapter
summary: A hosted TTS binding so a cascade can speak with a leading voice.
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T13:07:48Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-05T13:07:48Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-05T18:06:35Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Context

A downstream adopter is looking for the best voices for low-latency voice agents. On the public
blind TTS arena, ElevenLabs' Eleven v4 and v4 Turbo lead. v4 Turbo is reported at about 150 ms to
first speech over WebSocket streaming, with telephony output formats (`ulaw_8000`, `alaw_8000`).

inference has no hosted TTS adapter. The cascade can only bind the mock synthesiser, so a
cascade cannot be compared with a native speech model on voice or time to first audio.

## Planned scope

- A new adapter crate, `adapters/inference-elevenlabs`, behind its own feature, at locality
  `external_provider`.
- A streaming TTS binding of the `inference-tts` contract over ElevenLabs' WebSocket text input:
  - text in as it streams;
  - audio frames out, with the output format chosen, μ-law 8 kHz among them;
  - cancel on barge-in.
- A claims record held equal to the code, as the other hosted adapters have.
- The protocol taken from ElevenLabs' public API documentation, saved beside the fixtures with
  its date.
- Key from the environment or the keychain, never logged.

## Acceptance

- The backend-neutral TTS checks pass against a local WebSocket fixture server that speaks the
  documented protocol.
- First audio, cancel and a closed stream are each a case.
- The live test is ignored and behind feature `live`. It needs an ElevenLabs API key with TTS
  access.
