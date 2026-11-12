---
format: aep.planning-md/3
id: task:release-elevenlabs-route-p2
kind: task
status: draft
title: Release carrying the async speech handles
summary: One minor tag after the two P2 stories.
tags:
- downstream-input
relations:
- decomposes: epic:elevenlabs-route
- depends_on: story:async-speech-handles
- serves: vision:inference
revision: 2
---
## Context

The P2 work of epic:elevenlabs-route (the ElevenLabs agent's session overrides and usage, and the
async speech handles) reaches a phone voice-agent consumer only in a tag.

## Planned scope

- One minor release after story:elevenlabs-agent-session-overrides and story:async-speech-handles
  are on gated `main`, by the release rule of `AGENTS.md`, as task:release-elevenlabs-route-p1
  does it.
- Shared files: `Cargo.toml`, `CHANGELOG.md` and every `adapters/*/records/*.json`;
  story:duplex-session-config-additions depends on this task.

## Acceptance

- The tag exists on `origin` as an annotated tag on a `main` commit whose
  `inference / task check` and `Security and privacy` runs are green; `git merge-base
  --is-ancestor` shows the merge commits of story:elevenlabs-agent-session-overrides and
  story:async-speech-handles are ancestors of it; its `CHANGELOG.md` section has a `Breaking`
  heading naming the async handle change; its GitHub release notes passed `b10x-gates scan-text`.
