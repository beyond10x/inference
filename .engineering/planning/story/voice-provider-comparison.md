---
format: aep.planning-md/3
id: story:voice-provider-comparison
kind: story
status: active
title: Pick the voice provider by configuration and compare latency and usage across them
relations:
- decomposes: epic:native-voice
- serves: vision:inference
- depends_on: story:openai-gpt-live-adapter
- depends_on: story:realtime-voice-shows-usage
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T13:23:36Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-20T13:23:36Z", actor: "human:timo", revision: 3}
---
## Context

The owner, 2026-10-20: support OpenAI Realtime and GPT-Live side by side, "full pluggability, then
we can compare latency as well". `examples/realtime-voice` already talks to OpenAI Realtime and
Nova Sonic through one `inference-duplex` contract; the local cascade measures committed input to
first playable audio and interruption to playback stop.

## Planned scope

- `examples/realtime-voice`: `--provider openai-live` beside `openai` and `nova`, selected by
  configuration only, no other code path per provider.
- A `compare` example: the same recorded utterances (the committed question fixture and the
  LibriSpeech clip) sent through each configured provider (OpenAI Realtime, GPT-Live, Nova Sonic
  when its access is granted, and the local cascade when its adapters are present), N runs each,
  printing per provider: time from end of input speech to first output audio, to first transcript,
  to response end; barge-in to output stop (a second utterance sent while the answer plays); and
  usage per run (tokens or seconds as each provider bills). Median and spread over the runs; a
  provider without access is skipped with its reason.
- The measures are taken on the duplex events' own timestamps on the client side, named as such.

## Acceptance

`cargo test -p realtime-voice` passes with a mock-provider comparison test (two mock providers
with scripted delays give the expected ordering and figures); a live `compare` run against the
two OpenAI APIs prints the table, billed at most a few short exchanges.
