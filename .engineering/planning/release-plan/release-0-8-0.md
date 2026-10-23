---
format: aep.planning-md/3
id: release-plan:release-0-8-0
kind: release-plan
status: approved
title: Release 0.8.0
summary: 'Source release 0.8.0: three breaking changes in native speech usage, ess 0.68.0'
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-10-23T12:05:01Z", actor: "human:timo", revision: 2}
---
# Release 0.8.0

A source release of the workspace at the tag `0.8.0`, approved on 2026-10-23.

## Version

`CHANGELOG.md` Unreleased carries three **Breaking** entries (`SpeechSessionHandle::close`, provider
event fields as `Option`, `SessionLedger::response_usage_reported`), so the minor version moves:
0.7.0 to 0.8.0.

## Release commit

- `Cargo.toml` workspace version 0.8.0, `Cargo.lock` regenerated.
- `adapter_revision` 0.8.0 in every capability and claims record under `adapters/`.
- `CHANGELOG.md`: Unreleased becomes `[0.8.0] — 2026-10-23`.
- `README.md` status line; the site's status moves the four items on `main` to "Since 0.8.0", and
  the concept and guide pages that said "On main, in the next release" say "Since 0.8.0".
- `website/docs/getting-started.md`: dependency lines at tag 0.8.0 and an "Upgrading to 0.8.0"
  section for the three breaking changes.
- The `ess` pin stays at 0.68.0, the newest `ess` release on the release day.

## Steps

1. `task check` green on the release branch.
2. Pull request through the bot route; merge once the required checks pass.
3. Annotated tag `0.8.0` on the merge commit, pushed through the bot route.
4. GitHub Release from a notes file that passed `b10x-gates scan-text`.
