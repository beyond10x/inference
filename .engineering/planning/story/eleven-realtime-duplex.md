---
format: aep.planning-md/3
id: story:eleven-realtime-duplex
kind: story
status: draft
title: ElevenLabs realtime speech-to-speech without a hosted agent
summary: Eleven Realtime, the OpenAI-Realtime-compatible endpoint, as a duplex binding, settled first by a billed probe.
relations:
- decomposes: epic:elevenlabs-route
- serves: vision:inference
revision: 2
---
## Context

The operator wants ElevenLabs as raw text-to-speech, speech-to-text, and a realtime
speech-to-speech model in the manner of OpenAI Realtime, without a hosted ElevenLabs agent
configured in the ElevenLabs dashboard. ElevenLabs launched Eleven Realtime on 2026-10-20
(https://elevenlabs.io/blog/realtime, read 2026-11-12): speech-to-speech models
(`eleven_realtime_v1_mini`, and a Max model) behind `wss://api.elevenlabs.io/v1/convai/realtime`,
which the article says "follows the OpenAI Realtime specs"; its sample connects with no agent id.
The OpenAI realtime binding already takes another URL (`adapters/inference-openai/src/config.rs:113`,
`with_url`; default `wss://api.openai.com/v1/realtime` at `:15`).

Not known yet, to be settled by a billed probe before the design: the authentication header
(API key or a client secret), the audio formats (`pcm16`, `g711_ulaw`), the EU residency host, and
which OpenAI Realtime events it sends, notably usage.

## Planned scope

- A billed live probe, ignored, behind feature `live`: open a session on the EU host with the
  operator's key, send a short speech clip, record the events, the audio formats accepted and the
  usage reported.
- Then the smallest binding: either the OpenAI realtime duplex binding pointed at the ElevenLabs
  endpoint (a constructor or config naming the provider, its auth and its residency), or a thin
  ElevenLabs duplex binding sharing the OpenAI protocol code, whichever the probe allows.
- `talk --provider elevenlabs-realtime` and a `compare` arm.
- Docs: the provider and its variables; `CHANGELOG.md`.

## Acceptance

- A fixture test: a duplex session against a fixture server speaking the probed event set
  round-trips audio and records usage when the server reports it.
- A fixture test: μ-law 8 kHz input is accepted when the probe showed the endpoint accepts it,
  and refused as `Unsupported` at open when it does not.
- The billed probe exists and its output is recorded as test_result evidence.

## Status note

Parked on 2026-11-12: the consumer does not need a second speech-to-speech binding now; its
realtime voice path runs on the OpenAI Realtime binding. Not built, and no billed probe, until a
consumer asks for it.
