---
format: aep.planning-md/3
id: story:status-page-voice-adapters
kind: story
status: implemented
title: Status page and adapter reference list every hosted voice adapter with its first release
summary: Generated pages show ElevenLabs, Deepgram and Cartesia adapters with Since versions.
tags:
- downstream-input
relations:
- decomposes: epic:elevenlabs-route
- serves: vision:inference
- depends_on: story:elevenlabs-realtime-stt-adapter
- depends_on: task:release-elevenlabs-route-p1
- depends_on: story:elevenlabs-tts-provider-cancel
- depends_on: story:compare-elevenlabs-cascade-and-ollama
scope:
- confidence: cited
  path: crates/inference-docs/src/adapters.rs
- confidence: cited
  path: crates/inference-docs/src/status.rs
- confidence: cited
  path: website/data/status.json
- confidence: cited
  path: website/docs/reference/adapters.md
- confidence: cited
  path: website/docs/status.mdx
revision: 14
transitions:
- {from: "draft", to: "proposed", at: "2026-11-12T15:36:18Z", actor: "human:operator", revision: 12, decided_on: {"recorded":{"review_outcome":6}}}
- {from: "proposed", to: "active", at: "2026-11-12T15:36:18Z", actor: "human:operator", revision: 13, decided_on: {"recorded":{"review_outcome":6}}}
- {from: "active", to: "implemented", at: "2026-11-13T00:54:45Z", actor: "human:operator", revision: 14, decided_on: {"recorded":{"test_result":1,"review_outcome":7}}}
---
## Context

A phone voice-agent consumer reads the status page and the adapter reference to pick bindings
(epic:elevenlabs-route, outcome 4). `website/docs/reference/adapters.md` has no "Since" column on
`main` at `88f43b9c`.

## Planned scope

- `crates/inference-docs` generation of `website/docs/status.mdx`, `website/data/status.json` and
  `website/docs/reference/adapters.md`: every hosted voice adapter, ElevenLabs TTS, agent and STT,
  Deepgram STT and Voice Agent, Cartesia TTS, OpenAI Realtime, GPT-Live and speech, Nova Sonic,
  Phonic and Deepslate, each with the "Since 0.x.0" of the `CHANGELOG.md` section that introduced
  it.
- It depends on task:release-elevenlabs-route-p1, whose `CHANGELOG.md` section introduces
  ElevenLabs STT, and on story:elevenlabs-tts-provider-cancel and
  story:elevenlabs-realtime-stt-adapter, which that release waits on.

## Acceptance

- `task docs-check` passes.
- The generated `adapters.md` gives every row, hosted and local, a "Since" release.
- A test in `crates/inference-docs` checks each row's release against the `CHANGELOG.md` section
  that introduced that adapter, which the generator carries per binding.
- `website/data/status.json` lists each hosted voice adapter as shipped.
- Each of those `status.json` items names its release in its detail text.
- `status.json` items have the same set of fields as on `main` before this story.
- Each of those `status.json` items is held by an existing test that is not ignored.
- With tag `0.28.0` on `origin`, `adapters.md` and `status.json` show ElevenLabs realtime STT as
  "Since 0.28.0", and `task docs-check` passes on that tree.
