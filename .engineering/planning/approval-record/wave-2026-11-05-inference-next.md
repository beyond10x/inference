---
format: aep.planning-md/3
id: approval-record:wave-2026-11-05-inference-next
kind: approval-record
status: draft
title: 'Wave approved: voice providers (Deepgram STT and Voice Agent, ElevenLabs realtime, GPT-Live cut audio), release 0.17.0'
relations:
- decides: story:cut-response-audio-is-told-from-the-next-words
- decides: story:deepgram-streaming-stt-adapter
- decides: story:elevenlabs-realtime-agent-adapter
- decides: story:deepgram-voice-agent-adapter
revision: 1
---
## Decision

The coordinator approved this wave on 2026-11-05 under the orchestrator's grant for waves,
commits, pull requests and releases through the bot route. The input is a downstream adopter's
wish for low-latency voice agents with good barge-in and the best voices, and its November 2026
survey of voice providers.

The coordinator accepted these four stories for this wave, and they move from proposed to active
on its word:

- story:cut-response-audio-is-told-from-the-next-words, narrowed to GPT-Live;
- story:deepgram-streaming-stt-adapter;
- story:elevenlabs-realtime-agent-adapter;
- story:deepgram-voice-agent-adapter.

Accepted for the next wave (0.18.0), moved with this wave's close: story:deepslate-realtime-adapter,
story:elevenlabs-multi-context-tts-connection, story:phonic-realtime-adapter,
story:cartesia-streaming-tts-adapter. story:more-hosted-voice-adapters stays the draft umbrella.
story:blind-voice-preference-record stays later.

## Units

One unit builds at a time. U2 starts first, on this branch while 0.16.0 is open; U1, U3 and U4
start after 0.16.0 is on `main` and `main` is merged into this branch (no rebase).

| unit | story | surface | peak `target/` | cargo minutes |
|---|---|---|---|---|
| U1 | story:cut-response-audio-is-told-from-the-next-words | `examples/realtime-voice/src/lib.rs`, the GPT-Live binding, two test files | 8G | 12 |
| U2 | story:deepgram-streaming-stt-adapter | new `adapters/inference-deepgram`, new `inference-testkit` STT conformance | 6G | 15 |
| U3 | story:elevenlabs-realtime-agent-adapter | `adapters/inference-elevenlabs` feature `duplex`, `compare` | 10G | 18 |
| U4 | story:deepgram-voice-agent-adapter (depends on U2) | `adapters/inference-deepgram` agent module, `compare` | 10G | 18 |

- Estimates are readings of the dependency graph, not measurements. Each unit tells at 1.5 times
  its minutes and stops at twice them or at 12G.
- Review: one adversary pass per unit; a security-reviewer pass on U2 and U4; at most one
  correction each.
- Decisions taken at scoping: a provider end-of-turn maps onto commit of the open span (no new
  noun); the GPT-Live mark stays in the binding where it can; U2 builds the backend-neutral STT
  checks its acceptance names.
- Tests are fixture-only, from public protocol documentation fetched before each unit. No provider
  credential is requested; live checks are ignored tests behind feature `live`.

## Gate terms

| stage | content | peak | cargo minutes |
|---|---|---|---|
| s1 | sherpa archive, `ess specify validate`, `aep plan artifact validate`, docs `generate --check`, fmt | 0.2G | 0.1 |
| s2 | clippy, workspace, all targets and features | 1.6G | 2 |
| s3a | tests of the core crates, the ESS and STT conformance among them | 5.5G | 3 |
| s3b | tests of the examples and the facade | 4G | 3 |
| s3c | tests of the adapters | 6.5G | 4.5 |

- One pre-check per stage, at the stage's peak plus 40G (at least 45G free). Two build slots;
  `target/` is discarded after each stage.
- On green: one pull request through the bot, merged, tag `0.17.0`, release, release check.

## Commits authorised

One commit or more per unit with its review fixes, the merges into
`wave/2026-11-05-inference-next`, the merge of `main` after 0.16.0, the store close commit, one
pull request to `main`, and the tag `0.17.0`.
