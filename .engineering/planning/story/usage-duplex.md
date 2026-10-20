---
format: aep.planning-md/3
id: story:usage-duplex
kind: story
status: draft
title: 'Native speech usage: OpenAI Realtime and Nova Sonic responses and transcriptions as usage records'
summary: One usage record per response and per reported transcription, absent provider counts kept unknown.
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
- depends_on: story:usage-domain
revision: 1
---
## Context

Both hosted native speech adapters record usage through the `inference-duplex` ledger, but in a
form that loses what the provider did not report:

- OpenAI Realtime: `response.done` usage becomes `ResponseUsage` with every absent detail counted
  as 0 or folded into text (`adapters/inference-openai/src/duplex.rs:600-615`); transcription usage
  in tokens is recorded with absent counts as 0 (`src/event.rs:333-334`), and usage in seconds is
  dropped (`src/duplex.rs:32-33`).
- Nova Sonic: `usageEvent` deltas are summed per response (`adapters/inference-aws/src/duplex.rs:35`,
  482-492) into `ResponseUsage`.

A speech session runs in no graph run: nothing in `inference-duplex` names a run or node, and
`examples/realtime-voice` drives the session directly. So a native speech record names its speech
session and response instead.

## Planned scope

- `inference-duplex`: `SessionEvent::UsageRecorded(UsageRecorded)` and a ledger method that records
  one usage record per response when the response ends (completed, cancelled or failed), from the
  last running total the adapter gave, and one per transcription usage the provider reports.
  `ResponseUsageRecorded` and `TranscriptionUsageRecorded` stay as they are.
- Each adapter builds the quantities from the provider fields, not from `ResponseUsage`, so an
  absent field is `unknown`: text, audio and cached input tokens, text and audio output tokens; a
  transcription in tokens as `input_audio_tokens` and `output_text_tokens`, a transcription in
  seconds as `input_audio_ms`, `provider_reported`; `requests` 1 per response, `measured`. A
  response that ends with no usage reported yields a record whose token counts are all `unknown`.
- `inference-testkit` duplex mock: its scripted `Usage` step (`src/duplex/mock.rs`) feeds the same
  path, so the realtime-voice wiring tests see the records.
- `examples/realtime-voice`: the event match that ignores usage (`src/lib.rs:441-442`) counts the
  new event; nothing is printed.

## Domain relations

- UsageRecord to SpeechSession and to Response, each zero or one, to Run, Node and Session, each
  zero here: `ess/domains/usage.yaml`, `inference.usage.UsageRecord`, relations `speech_session`,
  `response`, `run`, `node`, `session`, as `story:usage-domain` declares them.
- UsageRecord to Adapter, one: the session adapter, `ess/domains/duplex.yaml`,
  `inference.duplex.SpeechSession`, relation `adapter`. UsageRecord to Model, one: that adapter
  model, `ess/domains/model.yaml`, `inference.model.Adapter`, relation `model`.

## Acceptance

Offline tests of both adapters over recorded provider events yield one usage record per response
and per reported transcription, with each count the provider sent as `provider_reported` and each
one it left out as `unknown` (an OpenAI transcription in seconds as `input_audio_ms`, a Nova response
as the sum of its deltas).

- A response cancelled by barge-in before any usage arrived yields a record with `unknown` counts.
- The duplex mock and the realtime-voice wiring tests see one record per response.
- `task check` passes; the live tests stay ignored.
