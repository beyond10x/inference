---
format: aep.planning-md/3
id: story:ess-pin-newest
kind: story
status: draft
title: The ess pin moves to the newest release in the next wave
relations:
- serves: vision:inference
revision: 1
---
## Context

Operator instruction (2026-10-22, at the wave-27 landing): the next wave moves the `ess` pin to the
newest release, under the workspace rule to always use the most recent ESS.

Where it stands at wave/27 `778363e`:

- CI pins `ESS_VERSION: "0.66.0"` (`.github/workflows/check.yml:51`) with its `ESS_SHA256`.
- `ess/ess-inputs.yaml:2` says `requires: ess 0.60`; the local gate runs `ess` 0.67.0 and warns
  that it is newer.
- The testkit's `ess-conformance` and `ess-primitives` dependencies are pinned by tag.
- The newest `ess` release is 0.67.0 (2026-10-21, `gh release list -R beyond10x/ess`). Re-read the
  newest release when the wave starts; it may have moved.

## Planned scope

- Move CI's `ESS_VERSION` and `ESS_SHA256`, the testkit's `ess-conformance` and `ess-primitives`
  tags and `ess-inputs.yaml`'s `requires` to the newest release, in one commit naming the version
  and the checksum (AGENTS.md, CI pins).
- Regenerate every generated ESS output and the conformance suite with that release; fix what the
  new release refuses. Anything beyond the pin, regeneration and small fixes is reported before
  it lands.
- One `CHANGELOG.md` line.

## Acceptance

- `task check` exits 0 with the newest `ess` and prints no "newer" warning for `ess-inputs.yaml`.
- CI's `check.yml` run on the wave's head is green with the new `ESS_VERSION`.
