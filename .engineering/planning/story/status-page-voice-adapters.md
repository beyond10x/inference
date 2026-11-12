---
format: aep.planning-md/3
id: story:status-page-voice-adapters
kind: story
status: draft
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
scope:
- confidence: cited
  path: crates/inference-docs
- confidence: cited
  path: website/data/status.json
- confidence: cited
  path: website/docs/reference/adapters.md
- confidence: cited
  path: website/docs/status.mdx
revision: 4
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

- `task docs-check` passes, and the generated `adapters.md` lists each adapter above with a
  "Since" equal to the first `CHANGELOG.md` section naming its binding type
  (`ElevenLabsSynthesiser`, `ElevenLabsAgent`, `ElevenLabsRecogniser`, `DeepgramRecogniser`,
  `DeepgramAgent`, `CartesiaSynthesiser`, `OpenAiRealtime`, `OpenAiLive`, `OpenAiSpeech`,
  `NovaSonicSpeech`, `PhonicSpeech`, `DeepslateOpal`).
