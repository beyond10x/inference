---
format: aep.planning-md/3
id: task:release-elevenlabs-route-p1
kind: task
status: active
title: 'Release carrying ElevenLabs STT, the provider-side TTS cancel and #39, with breaking changes per minor'
summary: One minor tag after the two P1 stories, and a page listing breaking changes per minor since 0.12.0.
tags:
- downstream-input
relations:
- decomposes: epic:elevenlabs-route
- depends_on: story:elevenlabs-realtime-stt-adapter
- depends_on: story:elevenlabs-tts-provider-cancel
- serves: vision:inference
- depends_on: story:talk-picks-elevenlabs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-12T09:37:36Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "proposed", to: "active", at: "2026-11-12T09:37:36Z", actor: "human:operator", revision: 5, decided_on: {"recorded":{"review_outcome":4}}}
---
## Context

A phone voice-agent consumer pins this repository by tag, fifteen minors back at `0.12.0`, and
moves to the newest tag. `main` at `88f43b9c` carries `#39` and no tag holds it; the newest tag is
`0.27.0`.

## Planned scope

- One minor release after story:elevenlabs-realtime-stt-adapter and
  story:elevenlabs-tts-provider-cancel are on gated `main`, by the release rule of `AGENTS.md`:
  version in `Cargo.toml`, `adapter_revision` of every capability record under `adapters/`,
  `CHANGELOG.md` entry with its `Breaking` section, the `ess` pin moved to the newest release,
  annotated tag and GitHub release through the bot route, notes scanned by
  `b10x-gates scan-text`.
- The `CHANGELOG.md` entry and the release notes carry a "Breaking since 0.12.0" list: one line
  per minor from `0.13.0` to the new one that had a `Breaking` section, each linking that section.
- Shared files: `Cargo.toml`, `CHANGELOG.md` and every `adapters/*/records/*.json`; the P2 and P3
  stories depend on this task so none edits them while it runs.

## Acceptance

- The tag exists on `origin` as an annotated tag on a `main` commit whose
  `inference / task check` and `Security and privacy` runs are green; `git merge-base
  --is-ancestor` shows the merge commits of `#39`, story:elevenlabs-realtime-stt-adapter and
  story:elevenlabs-tts-provider-cancel are ancestors of it; its `CHANGELOG.md` section has a
  `Breaking` heading and the "Breaking since 0.12.0" list, which has exactly one line per minor
  from `0.13.0` whose section has a `### Breaking` heading (`grep -n '^### Breaking'
  CHANGELOG.md`), each linking that section; its GitHub release notes passed
  `b10x-gates scan-text` and contain the same list.
