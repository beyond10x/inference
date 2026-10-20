---
format: aep.planning-md/3
id: story:realtime-voice-shows-usage
kind: story
status: active
title: realtime-voice prints usage per response and session totals
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T11:38:34Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-20T11:38:34Z", actor: "human:timo", revision: 3}
---
## Context

The owner, 2026-10-20, after a live run of `realtime-voice`: "could we maybe also display usage
there". The demo already receives `SessionEvent::ResponseUsageRecorded` and
`TranscriptionUsageRecorded` from the duplex binding and drops them
(`examples/realtime-voice/src/lib.rs:441-442`). The live run showed the OpenAI binding reports
input and output text, audio and cached tokens per response.

## Planned scope

- `examples/realtime-voice`: a `Note::Usage` per response (input and output text, audio and cached
  tokens) and per transcription; `talk` prints one compact line per response and, on exit, the
  session's totals and its wall duration. A count the provider did not report prints as `?`,
  never `0`.
- Optional `--prices <file>`: a small JSON price table (per 1M tokens: text in, cached in, text
  out, audio in, audio out) and `talk` prints the estimated cost per response and in total, the
  table's own name and date printed with it. No built-in prices. This is a demo-local stand-in until
  `story:usage-pricing` lands, which replaces it with `llm-cost` price documents.
- Tests with the duplex mock: usage notes are produced, totals equal the sum, an unreported count
  stays unknown, and the cost of a fixed table is computed exactly.

## Acceptance

`cargo test -p realtime-voice` passes with the new tests, and a live `talk --provider openai` run
prints a usage line per response and the totals at exit.
