---
format: aep.planning-md/3
id: approval-record:wave-2026-11-24-test-budget
kind: approval-record
status: draft
title: 'Wave approved: the test-run step under its 120 s budget, one pull request (2026-11-24)'
relations:
- decides: story:test-run-step-under-budget
revision: 1
---
## Decision

On 2026-11-24 the owner ordered, relayed by the operator: "Build: the over-budget test step first,
then the next wave from the active stories", and named the draft story of commit 1e3677c9c as the
one that comes before the next feature. That order is the acceptance of
story:test-run-step-under-budget and the approval of this wave. The coordinator ran the moves
draft to proposed to active on that order; it accepted nothing on its own.

Wave 1 of the trajectory `docs/handoff/2026-11-24-trajectory.md`. N is one.

| unit | story | objective | branch |
|---|---|---|---|
| U1 | story:test-run-step-under-budget | O4, and the test budget of the organization's agent rules | `impl/test-run-step-under-budget` |

## Check

`aep plan artifact waves --kind story --status active` at `745fe77b6`, before the story was
accepted, printed:

```
wave 1
  story:audio-device-adapter
  story:ess-defect-fixes
  story:signed-url-token-copies (inferred)
wave 2
  story:realtime-voice-demo (inferred)
collision: story:realtime-voice-demo story:signed-url-token-copies examples/realtime-voice/examples/compare.rs (inferred)
2 wave(s), 1 collision(s), 0 unassessed
```

None of those four stories is in this wave: story:audio-device-adapter and
story:realtime-voice-demo wait on a run with a microphone and a headset and plan no code,
story:ess-defect-fixes is blocked by a dependency, and story:signed-url-token-copies is in wave 2
of the trajectory. A wave of one has no pair to collide.

## Pre-flight

- The step this wave fixes, from the story: CI's `task test-run` took 154 s on the head `9950ff98`
  of pull request 61 against the 120 s budget; locally 448 s at a load average above 120, with 11
  tests of `inference-testkit` killed at 60 s.
- Free disk on the workstation: 51 G of 848 G (`df -h /`, 2026-11-24).
- The compiler cache is the host's shared one; builds and tests run through the shared build pool.
- The 14 worktrees of earlier waves were assessed `eligible` by `worktree gc --dry-run` and removed
  before this wave's first unit tree was created.

## Commits this approval authorises

One or more unit commits on `impl/test-run-step-under-budget`, the merge into
`wave/2026-11-24-test-budget`, the store commits of this wave (this record, the story's moves,
scope and evidence), the push of both branches, one pull request into `main` and its merge by the
operator when the checks are green. No tag, no version bump, no release.

## Integration

Branch `wave/2026-11-24-test-budget`, from `origin/main` at `745fe77b6`.

| unit | branch | worktree id | build directory | scratch | stage |
|---|---|---|---|---|---|
| U1 | `impl/test-run-step-under-budget` | `inference-u1-test-budget` | `target/` inside the unit's worktree | the unit's directory under the session scratch | planned |
