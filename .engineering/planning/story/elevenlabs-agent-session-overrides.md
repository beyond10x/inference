---
format: aep.planning-md/3
id: story:elevenlabs-agent-session-overrides
kind: story
status: archived
title: ElevenLabs agent sends the session's prompt, first message, voice and language, and records what usage it can
summary: conversation_config_override from the neutral session configuration; usage recorded or documented as unknown.
tags:
- downstream-input
relations:
- decomposes: epic:elevenlabs-route
- serves: vision:inference
- depends_on: task:release-elevenlabs-route-p1
- depends_on: story:elevenlabs-realtime-stt-adapter
- depends_on: story:elevenlabs-tts-provider-cancel
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: inferred
  path: adapters/inference-aws/src
- confidence: inferred
  path: adapters/inference-deepgram/src
- confidence: inferred
  path: adapters/inference-deepslate/src
- confidence: cited
  path: adapters/inference-elevenlabs/records/claims-agent.json
- confidence: cited
  path: adapters/inference-elevenlabs/src/agent.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/agent_duplex.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests
- confidence: inferred
  path: adapters/inference-openai/src
- confidence: inferred
  path: adapters/inference-phonic/src
- confidence: cited
  path: crates/inference-duplex/src/contract.rs
- confidence: inferred
  path: crates/inference-testkit/src/duplex
- confidence: cited
  path: ess/domains/duplex.yaml
revision: 7
transitions:
- {from: "draft", to: "archived", at: "2026-11-12T12:32:16Z", actor: "human:operator", revision: 7, decided_on: {"recorded":{"review_outcome":6}}}
---
## Context

The ElevenLabs agent binding sends neither the session's voice nor its instructions: "the agent's
own apply" (`adapters/inference-elevenlabs/src/agent_duplex.rs:23-24`). A phone voice-agent
consumer owns prompt and flow per call. The neutral `SessionConfig`
(`crates/inference-duplex/src/contract.rs:35-46`) has voice and instructions but no first message
and no conversation language. The binding also records no usage: `git grep
ResponseUsageRecorded` finds none in `agent_duplex.rs` and one in
`adapters/inference-openai/src/duplex.rs`.

## Planned scope

- Contract, `ess/domains/duplex.yaml` first, then `crates/inference-duplex/src/contract.rs`: two
  optional `SessionConfig` fields, a first message (what the model says before the caller speaks)
  and a conversation language (BCP 47). A binding that cannot honour one refuses it at open as
  `Unsupported`; every duplex binding in the repository states which it takes, in the same change.
  The conversation language is distinct from the transcription language of
  story:duplex-session-config-additions.
- `adapters/inference-elevenlabs/src/agent.rs` and `agent_duplex.rs`: send
  `conversation_initiation_client_data` with `conversation_config_override` built from the session
  configuration (instructions as the prompt, first message, voice, language) as the first client
  message. The protocol is read again from ElevenLabs' documentation and the saved summary updated
  with its date.
- Usage: where the documented protocol reports usage, the binding records it as
  `ResponseUsageRecorded` or `TranscriptionUsageRecorded`; where it reports none, the crate doc and
  `records/claims-agent.json` say usage is unknown, and no record with zero counts is emitted.
- Each binding states the two fields as `SpeechCapabilities` flags, as
  story:duplex-session-config-additions does for its controls.
- Shared files: `agent_duplex.rs`, `tests/` and `records/claims-agent.json` with
  story:duplex-session-config-additions and story:async-speech-handles, which depend on this
  story; `adapters/inference-elevenlabs/tests`, `records/` and `CHANGELOG.md` with
  story:elevenlabs-realtime-stt-adapter and story:elevenlabs-tts-provider-cancel, on which this
  story depends, as on task:release-elevenlabs-route-p1.

## Acceptance

- A fixture test: the first client message carries `conversation_config_override` with the
  session's instructions, first message, voice and language.
- A fixture test: a session with none of the four sends no override.
- A fixture test: the OpenAI Realtime binding, whose protocol has no first message, refuses a
  session that sets one at open as `Unsupported`.
- A test asserts, for every duplex binding in the repository and the testkit mock, the value of
  both new capability flags.
- Usage: the crate doc names the ElevenLabs documentation page and the date it was read, and
  states what the protocol reports about usage; when it documents a usage message, a fixture test
  shows that message yields a usage record with its counts; when it documents none, a fixture test
  shows a full conversation yields no usage record, and `records/claims-agent.json` says usage is
  unknown.
- `agent_capabilities` still claims no manual interrupt, cancel or truncation.
