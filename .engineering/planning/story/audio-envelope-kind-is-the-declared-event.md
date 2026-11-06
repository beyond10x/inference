---
format: aep.planning-md/3
id: story:audio-envelope-kind-is-the-declared-event
kind: story
status: implemented
title: Audio envelopes carry the declared event name inference.audio.AudioChunk
summary: Every envelope of a frame of audio is stamped inference.audio.AudioChunk, the declared event, as SynthesisEvent::name() already returns since 0.19.0; no alias for the old kind
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
- serves: vision:inference
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T19:11:12Z", actor: "human:operator", revision: 4}
- {from: "proposed", to: "active", at: "2026-11-06T19:11:13Z", actor: "human:operator", revision: 5}
- {from: "active", to: "implemented", at: "2026-11-06T21:20:41Z", actor: "human:operator", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
# Story: audio envelopes carry the declared event name

## Outcome

A consumer that routes pipeline envelopes by kind sees one name for a frame of synthesized or
captured audio, `inference.audio.AudioChunk`, the event `ess/domains/audio.yaml` declares, and no
envelope anywhere is stamped `inference.audio.AudioFrame`, which is the type that holds the frame.

## Context

0.19.0 (finding N14 of review-result:full-codebase-2026-11-06) made `SynthesisEvent::name()` return
`inference.audio.AudioChunk` for a frame. About 25 files still stamp envelopes with the kind
`inference.audio.AudioFrame`: the TTS testkit node, `inference-audio-device`'s `AUDIO_FRAME`, the
runtime's record, the testkit cascade, `local-voice-cascade` and their tests (43 occurrences by
`rg -c 'inference\.audio\.AudioFrame' -g '!target'` on 0.22.0). Backwards compatibility is not
kept (AGENTS.md, 2026-11-06): no deprecated alias.

## Acceptance

- `rg 'inference\.audio\.AudioFrame' -g '!target'` matches only the type's own declaration and
  doc lines (the struct in `inference-audio` and `ess/domains/audio.yaml`, and fields typed
  `inference.audio.AudioFrame`), never an envelope kind or a constant used as one.
- A test asserts that the TTS node and the audio-device node each emit envelopes whose kind equals
  `SynthesisEvent` frame's `name()`, `inference.audio.AudioChunk`.
- `CHANGELOG.md` `## [Unreleased]` has a `Breaking` line naming every renamed or removed constant.

## Out of Scope

- Renaming the type `AudioFrame`.
