---
format: aep.planning-md/3
id: task:release-async-speech-handles
kind: task
status: draft
title: 'Release 0.30.0: the async form of the speech handles'
summary: One minor tag after story:async-speech-handles merged on 2026-11-13.
relations:
- decomposes: epic:elevenlabs-route
- serves: vision:inference
- depends_on: story:async-speech-handles
revision: 1
---
## Context

story:async-speech-handles reaches a consumer that runs tokio only in a tag. It merged to `main`
as 0303510c on 2026-11-13 (PR #52). Its release was granted on the operator's behalf on
2026-11-13 for one tag, 0.30.0, from that commit plus this store change and the release commit.

## Planned scope

- One minor release, 0.30.0, following the release rule of `AGENTS.md` as
  task:release-elevenlabs-route-p2 does: version in the workspace manifest, lock file, README,
  the site's install pages and every capability record's `adapter_revision`; the `[Unreleased]`
  section of `CHANGELOG.md` becomes `[0.30.0]`; the `ess` pin moves to the newest `ess` release.
- Nothing else lands on `main` between 0303510c and the tag.
- Shared files: `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md` and every `adapters/*/records/*.json`.

## Acceptance

- The annotated tag `0.30.0` exists on `origin` on a `main` commit whose `inference / task check`,
  `common / Security and privacy` and `Build documentation` runs are green.
- `git merge-base --is-ancestor 0303510c 0.30.0` succeeds.
- `CHANGELOG.md` has a `## [0.30.0]` section with no `### Breaking` heading; the `[0.29.0]`
  section is byte-identical to tag 0.29.0.
- The GitHub release notes passed `b10x-gates scan-text`; they name the async forms, each
  adapter's wake behaviour as the tests show it, and what was not run against a live provider.
