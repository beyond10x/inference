---
format: aep.planning-md/3
id: release-plan:release-0-8-1
kind: release-plan
status: approved
title: Release 0.8.1
summary: 'Source release 0.8.1: ess 0.76.0 pin and one playout fix, no breaking change'
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-10-29T09:58:28Z", actor: "human:timo", revision: 2}
---
# Release 0.8.1

A source release of the workspace at the tag `0.8.1`, approved by the operator on 2026-10-29 as one
wave: the three active stories re-checked, the `ess` pin moved to the newest release, one release.

## Version

`CHANGELOG.md` Unreleased carries no **Breaking** entry: the `ess` pin moves only CI and the
testkit's dev-dependencies, and the `Speaker::push` fix answers with the existing
`PushError::Stopped`. The patch version moves: 0.8.0 to 0.8.1.

## Wave

- `story:audio-device-adapter` and `story:realtime-voice-demo`: built on `main`; what remains is
  their device acceptance on a laptop, which no code clears. Left out, recorded in each story.
- `story:ess-defect-fixes`: still blocked by `dependency-blocker:ess-affects-witness` under ess
  0.76.0 (1 synthesis refusal, 1 unsupported interpreted scenario). Left out, recorded in the
  story and the blocker.
- Ride-along: CI, `ess-inputs.yaml` and the testkit's `ess-conformance` and `ess-primitives` move
  from 0.75.0 to 0.76.0.

## Release commit

- `Cargo.toml` workspace version 0.8.1, `Cargo.lock` regenerated.
- `adapter_revision` 0.8.1 in every capability and claims record under `adapters/`.
- `CHANGELOG.md`: Unreleased becomes `[0.8.1] — 2026-10-29`.
- `README.md` status line; `website/docs/getting-started.md` dependency lines at tag 0.8.1; the
  generated status page names 0.8.1.

## Steps

1. `task check` green on the wave branch.
2. Pull request through the bot route; merge once the required checks pass.
3. Annotated tag `0.8.1` on the merge commit, pushed through the bot route.
4. GitHub Release from a notes file that passed `b10x-gates scan-text`.
