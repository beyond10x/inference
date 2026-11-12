---
format: aep.planning-md/3
id: task:release-elevenlabs-route-p2
kind: task
status: active
title: 'Release 0.29.0: async speech handles, status page, cascade latency comparison'
summary: One minor tag after the five P2 items.
tags:
- downstream-input
relations:
- decomposes: epic:elevenlabs-route
- depends_on: story:async-speech-handles
- serves: vision:inference
- depends_on: story:status-page-voice-adapters
- depends_on: story:realtime-voice-barge-in-stop-flake
- depends_on: story:compare-elevenlabs-cascade-and-ollama
- depends_on: task:voice-latency-comparison-run
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-11-12T15:36:19Z", actor: "human:operator", revision: 7}
- {from: "proposed", to: "active", at: "2026-11-12T15:36:20Z", actor: "human:operator", revision: 8}
---
## Context

Wave P2 of epic:elevenlabs-route reaches a phone voice-agent consumer only in a tag. It holds the
async speech handles, the status page, a fix to an intermittent test, and a measured comparison
of an ElevenLabs cascade with the realtime arms (approval-record:wave-elevenlabs-route-p2). The
agent overrides first planned here were archived on 2026-11-12.

## Planned scope

- One minor release, 0.29.0, after story:async-speech-handles,
  story:status-page-voice-adapters, story:realtime-voice-barge-in-stop-flake,
  story:compare-elevenlabs-cascade-and-ollama and task:voice-latency-comparison-run are on gated
  `main`. It follows the release rule of `AGENTS.md`, as task:release-elevenlabs-route-p1 does.
- Shared files: `Cargo.toml`, `CHANGELOG.md` and every `adapters/*/records/*.json`.
  story:duplex-session-config-additions depends on this task.

## Acceptance

- The annotated tag `0.29.0` exists on `origin` on a `main` commit whose `inference / task check`
  and `Security and privacy` runs are green.
- `git merge-base --is-ancestor` shows the merge commits of story:async-speech-handles,
  story:status-page-voice-adapters, story:realtime-voice-barge-in-stop-flake,
  story:compare-elevenlabs-cascade-and-ollama and task:voice-latency-comparison-run are
  ancestors of the tag.
- `CHANGELOG.md` has a `## [0.29.0]` section. It carries a `### Breaking` heading only if a public
  type of the wave changed; otherwise it has none.
- The GitHub release notes passed `b10x-gates scan-text`.
