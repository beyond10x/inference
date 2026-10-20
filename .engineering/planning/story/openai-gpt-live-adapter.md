---
format: aep.planning-md/3
id: story:openai-gpt-live-adapter
kind: story
status: active
title: OpenAI GPT-Live (v1/live/sessions) as an inference-duplex native speech model
relations:
- decomposes: epic:native-voice
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T11:40:52Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-20T11:40:52Z", actor: "human:timo", revision: 3}
---
## Context

The owner, 2026-10-20: "we do want to support both of them, full pluggability, then we can compare
latency as well". OpenAI ships two voice APIs: Realtime (`v1/realtime`, `gpt-realtime-2.1`, one
model for speech, reasoning and tools, billed per token), which `adapters/inference-openai`
implements, and Live (`v1/live/sessions`, `gpt-live-1`, a full-duplex voice model that delegates
reasoning and tools to a backend agent, billed per second of session, backend billed apart). Read
on 2026-10-20: https://developers.openai.com/api/docs/models/gpt-live-1.md and
https://developers.openai.com/api/docs/guides/live (WebRTC recommended, WebSocket supported, a
server-side sideband WebSocket; delegation mode "responses" or "client", chosen per session).

## Planned scope

- First, read the Live API reference (session creation, the event set, audio formats, delegation
  configuration, usage and billing events) and write what was read, with URLs and the date, into
  the module doc. Anything the docs leave open is named, not guessed.
- `adapters/inference-openai`: a `live` client module beside the Realtime client (WebSocket
  transport; WebRTC is out of scope), behind a feature `gpt-live` (not `live`, which already names
  the live-test feature), sharing the credential sources (`openai-api-key`).
- An `inference-duplex` binding for it, so `gpt-live-1` is one native speech node like the Realtime
  model: audio in and out, transcripts, barge-in, function tools through Responses delegation
  (OpenAI runs the backend model; this application runs its own function tools). Its declared
  capabilities are what the conformance probe observes; `native_duplex` is claimed only if the
  probe shows input during output changes the output.
- Usage: session duration as the billed quantity, plus whatever usage the API reports for the
  backend, mapped onto the duplex usage events (and onto `inference.usage` once
  `story:usage-duplex` lands); nothing reported is never written as 0.
- Offline tests over recorded or documented event sequences; live tests ignored, behind the
  `live` feature, reading the key from the keychain; billed: keep them short.

## Acceptance

`cargo test -p inference-openai --all-features` passes offline; with the key in the keychain,
`cargo test -p inference-openai --features live,gpt-live -- --ignored` runs one spoken exchange
through the duplex contract on `gpt-live-1`, prints its usage, and the native-duplex probe result
matches the declared claim.
