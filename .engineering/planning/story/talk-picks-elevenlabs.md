---
format: aep.planning-md/3
id: story:talk-picks-elevenlabs
kind: story
status: implemented
title: talk picks the ElevenLabs agent or an ElevenLabs cascade, and ElevenLabs endpoints pick their data-residency host
summary: 'Two talk providers to try the ElevenLabs route against the real API: the agent, and ElevenLabs STT, the local LLM and ElevenLabs TTS.'
tags:
- operator-request
relations:
- decomposes: epic:elevenlabs-route
- serves: vision:inference
- depends_on: story:elevenlabs-tts-provider-cancel
- depends_on: story:elevenlabs-realtime-stt-adapter
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: adapters/inference-elevenlabs/examples/live.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/config.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/agent_duplex_live.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/stt_live.rs
- confidence: inferred
  path: examples/realtime-voice/Cargo.toml
- confidence: cited
  path: examples/realtime-voice/examples/compare.rs
- confidence: cited
  path: examples/realtime-voice/examples/talk.rs
- confidence: inferred
  path: examples/realtime-voice/src/lib.rs
- confidence: cited
  path: examples/realtime-voice/tests
- confidence: cited
  path: website/docs/guides/talk-to-a-hosted-voice-model.md
- confidence: cited
  path: website/docs/reference/environment.md
revision: 14
transitions:
- {from: "draft", to: "proposed", at: "2026-11-12T11:16:52Z", actor: "human:operator", revision: 4}
- {from: "proposed", to: "active", at: "2026-11-12T11:16:52Z", actor: "human:operator", revision: 5}
- {from: "active", to: "implemented", at: "2026-11-12T13:27:54Z", actor: "human:operator", revision: 14, decided_on: {"recorded":{"test_result":1,"review_outcome":2}}, correlation: "wave-elevenlabs-route-p1"}
---
## Context

The operator wants to try the ElevenLabs route against the real API from a demo. `talk` offers
only `openai`, `openai-live` and `nova` (`examples/realtime-voice/examples/talk.rs:69-76`); only
`compare` reaches ElevenLabs, as its agent arm and as the cascade's voice
(`examples/realtime-voice/examples/compare.rs:19-40`). Nothing uses the ElevenLabs recogniser of
story:elevenlabs-realtime-stt-adapter outside its own tests.

The first live run with the operator's key (2026-11-12) failed on the global host: the key belongs
to the EU data-residency stack ("The API key used is for a data residency stack, however this
server is a global server"). The same text-to-speech and speech-to-text checks passed with the
endpoint set to `wss://api.eu.residency.elevenlabs.io`. The agent binding has `Residency`
(`adapters/inference-elevenlabs/src/agent.rs:100-123`); the TTS and STT `Endpoint` has only
`with_url` (`src/config.rs:58`), and no example or live test sets it.

## Planned scope

- Residency (`Endpoint::with_residency`, `Endpoint::from_env` reading `ELEVENLABS_RESIDENCY`) was
  built in story:elevenlabs-tts-provider-cancel (merged into the wave at be16f689), because its
  billed probe needed the EU host. What remains here: `compare` (its ElevenLabs cascade voice at
  `examples/realtime-voice/examples/compare.rs:391`, `:513`, `:518` uses `Endpoint::new()`,
  review-result:adversary-elevenlabs-tts-cancel-pass-2 finding F6), `talk`, and the agent live
  test use `Endpoint::from_env()` or the agent's residency from the same variable.

- `examples/realtime-voice/examples/talk.rs`: one more provider (the ElevenLabs agent provider was dropped on 2026-11-12: the operator does not use the hosted ElevenLabs agent).
  - `elevenlabs-cascade`: speech-to-text with `ElevenLabsRecogniser` (VAD commit, `pcm_16000` from
    the microphone), the LLM the local voice cascade configures
    (`local_voice_cascade::local::LocalConfig::from_env`, feature `local`), and text-to-speech
    with `ElevenLabsSynthesiser` (`ELEVENLABS_VOICE_ID`, `pcm_24000`), with barge-in cancelling
    the segment being spoken.
- A missing variable fails with exit status 2 naming it, as the other providers do.
- `website/docs/guides/talk-to-a-hosted-voice-model.md` and `website/docs/reference/environment.md`
  name the provider and its variables; `CHANGELOG.md` `## [Unreleased]`.
- Shared files: `examples/realtime-voice` with story:elevenlabs-tts-provider-cancel (its latency
  report), on which this story depends.

## Acceptance

- A unit test of the residency in the binding was delivered by story:elevenlabs-tts-provider-cancel
  (`Endpoint::from_env`: `eu` gives `wss://api.eu.residency.elevenlabs.io`, unset gives the global
  host, `mars` is refused naming the variable).
- The ElevenLabs speech-to-text and text-to-speech endpoints `compare` and `talk` build reach the
  host `ELEVENLABS_RESIDENCY` names (the hosted-agent arm of `compare` is out of scope: the
  operator does not use the hosted agent; review-result:adversary-talk-elevenlabs-pass-1 F6): a test in
  `examples/realtime-voice/tests` shows the endpoint they build follows the variable.
- `cargo run -p realtime-voice --example talk -- --help` lists
  `elevenlabs-cascade`.
- A test of the provider table in `examples/realtime-voice/tests` shows the new provider exits 2
  naming its first missing variable when it is unset.
- The live run with a microphone is the operator's to try; it is not a gate of this story.
