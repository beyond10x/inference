---
format: aep.planning-md/3
id: story:elevenlabs-multi-context-tts-connection
kind: story
status: implemented
title: ElevenLabs TTS speaks an utterance on one connection
summary: Bind the multi-stream endpoint so segments after the first pay no handshake and a barge-in closes a context, not a connection
tags:
- downstream-input
relations:
- decomposes: epic:native-voice
- serves: vision:inference
- depends_on: story:elevenlabs-streaming-tts-adapter
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T17:26:37Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-06T17:26:37Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-06T18:22:07Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Context

The ElevenLabs streaming TTS adapter (story:elevenlabs-streaming-tts-adapter) speaks each segment
on a WebSocket connection of its own. The single-stream endpoint has no marker between flushes;
its only end marker, `isFinal`, follows the end of input. So the adapter sends the end of input
after each segment, and every segment after the first pays one connection handshake before its
first audio.

ElevenLabs' multi-stream endpoint keeps one connection with several contexts. It flushes and
closes a context by its `context_id`, and it marks the end of each context with `isFinal`. A
barge-in closes the speaking context, without opening a new connection.

## Planned scope

- Add a binding over the multi-stream endpoint, with one connection per synthesis and one context
  per segment, or replace the per-segment binding with it.
- Map cancel to `close_context`, and the end of the utterance to `close_socket`.
- The fixture server speaks the multi-stream protocol taken from the public documentation, as the
  single-stream fixture does.

## Acceptance

- The backend-neutral TTS checks pass against the fixture server.
- In a fixture run of three segments, one connection is opened, against three today.
- A cancel mid-segment yields no further frame of that segment, and the next segment speaks on
  the same connection.
