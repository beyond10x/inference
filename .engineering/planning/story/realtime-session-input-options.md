---
format: aep.planning-md/3
id: story:realtime-session-input-options
kind: story
status: implemented
title: 'Realtime session config: noise reduction, transcription language and prompt, tool choice'
summary: SessionConfig sends audio.input.noise_reduction, transcription language and prompt, and session tool_choice.
tags:
- downstream-adopter
- openai-realtime
relations:
- decomposes: epic:native-voice
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-openai/src/config.rs
- confidence: cited
  path: adapters/inference-openai/src/duplex.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-03T11:15:59Z", actor: "human:operator", revision: 2, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
- {from: "proposed", to: "active", at: "2026-11-03T11:15:59Z", actor: "human:operator", revision: 3, executor: "agent:inference-wave-0-10-0", correlation: "wave-0-10-0"}
- {from: "active", to: "implemented", at: "2026-11-03T18:16:40Z", actor: "human:operator", revision: 6, decided_on: {"recorded":{"test_result":2,"review_outcome":1}}}
---
## Context

A downstream adopter reported that `SessionConfig` cannot express input noise reduction, a
transcription language or prompt, or a session tool choice
(`adapters/inference-openai/src/config.rs:136-149` on 0.9.0, verified: the struct holds
instructions, voice, output modality, input and output format, turn detection, the transcription
model, tools and speed; `to_session_json` writes `audio.input.transcription` as `{"model": ...}`
only, `config.rs:186`).

## Planned scope

- `adapters/inference-openai/src/config.rs`: `SessionConfig` gains input noise reduction
  (near field, far field, or off as an explicit `null`), the transcription language and prompt
  (sent inside `audio.input.transcription` beside the model), and a session `tool_choice`
  (`auto`, `none`, `required`, or one named function). A `ToolChoice` type, public, which
  story:realtime-response-options reuses. Every new field defaults to "left out", so a 0.9.0
  configuration sends the same `session.update`.
- `adapters/inference-openai/src/duplex.rs`: the binding's `SessionConfig` literal fills the new
  fields with their defaults.

## Acceptance

Named scenarios, unit tests on `SessionConfig::to_session_json` and offline tests:

- `noise_reduction_is_sent_under_audio_input`: near field and far field send
  `audio.input.noise_reduction: {"type": "near_field" | "far_field"}`; off sends `null`; unset
  leaves the key out.
- `transcription_language_and_prompt_are_sent_with_the_model`: both appear inside
  `audio.input.transcription` beside `model`.
- `session_tool_choice_is_sent`: each `ToolChoice` reaches `session.tool_choice` in the
  reference's shape (a string, or `{"type": "function", "name": ...}`).
- `default_session_json_unchanged`: `SessionConfig::default()` serialises as it did on 0.9.0.

## Cause

Verified in code: `config.rs:136-149` and `config.rs:176-205` on 0.9.0 have no field and no
serialisation for any of the four.
