---
format: aep.planning-md/3
id: story:duplex-session-config-additions
kind: story
status: draft
title: 'Duplex session configuration: per-response instructions, item placement, transcription settings, speed, truncation'
summary: Neutral controls a realtime voice loop uses, each behind a capability flag.
tags:
- downstream-input
relations:
- decomposes: epic:elevenlabs-route
- serves: vision:inference
- depends_on: task:release-elevenlabs-route-p2
- depends_on: story:elevenlabs-agent-session-overrides
- depends_on: story:async-speech-handles
scope:
- confidence: inferred
  path: adapters/inference-aws/src/duplex.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/agent_duplex.rs
- confidence: inferred
  path: adapters/inference-deepslate/src/opal.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/agent_duplex.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests
- confidence: cited
  path: adapters/inference-openai/src/duplex.rs
- confidence: inferred
  path: adapters/inference-openai/src/live_duplex.rs
- confidence: cited
  path: adapters/inference-openai/tests
- confidence: inferred
  path: adapters/inference-phonic/src/duplex.rs
- confidence: cited
  path: crates/inference-duplex
- confidence: cited
  path: crates/inference-testkit/src/duplex
- confidence: cited
  path: crates/inference/src/stack.rs
- confidence: cited
  path: ess/domains/duplex.yaml
revision: 6
---
## Context

A phone voice-agent consumer drives OpenAI Realtime through its own adapter and cannot move it
onto `inference-duplex` because the neutral session configuration lacks controls its voice loop
uses. Each control below is a message or session field of the OpenAI Realtime protocol.

## Planned scope

- `ess/domains/duplex.yaml` first, then `crates/inference-duplex`: per-response instructions;
  placement of a conversation item after a given item; transcription model, language and
  vocabulary prompt; speech speed; truncation of an output item by item id and milliseconds.
- Each one a capability flag in `SpeechCapabilities`, so a binding that lacks it refuses it as
  `Unsupported` at open or at the call, never ignores it.
- `adapters/inference-openai/src/duplex.rs` implements all five; every other duplex binding (GPT-
  Live, Nova Sonic, ElevenLabs agent, Deepgram agent, Phonic, Deepslate, the testkit mock) states
  its flags as false unless its protocol has the control.
- Worked alone: `SpeechCapabilities` is built by seven adapters, the testkit mock and
  `crates/inference/src/stack.rs`. It depends on task:release-elevenlabs-route-p2 and directly on
  story:elevenlabs-agent-session-overrides and story:async-speech-handles, which share
  `agent_duplex.rs`, `contract.rs`, `ess/domains/duplex.yaml` and
  `adapters/inference-elevenlabs/tests` with it.
- Tests: `adapters/inference-openai/tests` for the five fixture tests,
  `adapters/inference-elevenlabs/tests/agent_duplex.rs` for the refusals.

## Acceptance

- `ess specify validate --path ess` passes with the additions.
- One OpenAI Realtime fixture test per addition (five) shows the client message the protocol
  documents for it.
- One refusal test per addition on the ElevenLabs agent fixture shows `Unsupported` and no message
  sent.
