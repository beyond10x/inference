---
format: aep.planning-md/3
id: approval-record:wave-2026-11-24-dub-voice
kind: approval-record
status: draft
title: 'Wave approved: one-call speak, a local synthesiser and two ElevenLabs fixes, one pull request (2026-11-24)'
relations:
- decides: story:sdk-one-call
- decides: story:synthesiser-drop-inside-runtime
- decides: story:local-tts-adapter-under-licence-policy
- decides: story:signed-url-token-copies
revision: 4
---
## Decision

On 2026-11-24 the coordinator put the question which drafts become active for the next wave to the
operator, with option A: accept story:sdk-one-call and story:synthesiser-drop-inside-runtime, the
wave being those two and story:signed-url-token-copies. The operator answered at 13:47 CET:
"D2 = A [...] accept story:sdk-one-call and story:synthesiser-drop-inside-runtime; wave 2 = those 2
plus story:signed-url-token-copies. Reason: the consumer needs a one-call `speak`, and it is on a
branch."

In the same message the operator passed on the gaps a consumer reported (a screencast dub that
needed a local voice and could not use this repository) and ordered: "File them as stories and
order wave 2 by them where they fit; (1) is new scope and a decision of yours to report, not to
ask, unless it changes the architecture." Gap (1) is a local synthesiser behind the same contract
as the hosted ones, which is story:local-tts-adapter-under-licence-policy. The coordinator ran the
moves draft to proposed to active for the three drafts on these words; it accepted nothing on its
own.

Wave 2 of the trajectory `docs/handoff/2026-11-24-trajectory.md`, which this wave's first commit
reorders by the consumer's gaps.

| unit | story | objective | branch |
|---|---|---|---|
| U1 | story:sdk-one-call | O1 | `impl/sdk-one-call` |
| U2 | story:synthesiser-drop-inside-runtime | O4 | `impl/synthesiser-drop-inside-runtime` |
| U3 | story:signed-url-token-copies | O3 | `impl/signed-url-token-copies` |
| U4 | story:local-tts-adapter-under-licence-policy | O1 | `impl/local-tts-adapter` |

## The decision on gap (1), taken by the coordinator and reported

The local synthesiser is built on sherpa-onnx offline TTS in its own opt-in crate, the route the
owner chose on 2026-10-22 (approval-record:sherpa-tts-gpl-phonemiser). It runs VITS voices in the
Piper format, which is what the consumer used. A synthesiser run as an external process is not what
that exception names and is not taken. No architecture changes: the adapter is one more crate under
`adapters/` that depends on the TTS contract.

## Gaps filed as drafts, not in this wave

story:speak-text-file-to-audio-file (gap 2), story:sentence-clips-with-durations (gap 3) and
story:spoken-form-check-before-synthesis (gap 4). Gaps 2 and 3 build on U1 and U4 and are the next
wave of the trajectory once the operator accepts them.

## Check

`aep plan artifact waves --kind story --status active` on the wave's branch, run again after the
scope of story:local-tts-adapter-under-licence-policy was written, printed:

```
wave 1
  story:audio-device-adapter
  story:ess-defect-fixes
  story:local-tts-adapter-under-licence-policy (inferred)
  story:sdk-one-call (inferred)
  story:signed-url-token-copies (inferred)
wave 2
  story:realtime-voice-demo (inferred)
  story:synthesiser-drop-inside-runtime (inferred)
collision: story:signed-url-token-copies story:synthesiser-drop-inside-runtime CHANGELOG.md (inferred)
collision: story:signed-url-token-copies story:synthesiser-drop-inside-runtime adapters/inference-elevenlabs/src/lib.rs
unassessed: story:sentence-clips-with-durations
unassessed: story:speak-text-file-to-audio-file
2 wave(s), 2 collision(s), 2 unassessed
```

- U2 and U3 collide on `adapters/inference-elevenlabs/src/lib.rs` (U2 lines 41 and 52-55, U3 lines
  71-88) and on `CHANGELOG.md`. They do not run at once: U3 runs first, and U2 is forked from the
  integration branch after U3 has merged into it.
- U4 is placed beside U1 and U3 and starts now.
- `CHANGELOG.md` is the coordinator's: each unit hands its lines over in its report.
- `website/docs/reference/environment.md` is held for the coordinator where U4 needs it: U3 edits
  one sentence there, U4 hands its text over.
- `crates/inference-docs/src/status.rs` is U4's (the planned entry "Local text-to-speech"). The
  entry was removed from the stored scope of story:sdk-one-call: U1's brief does not assign the
  file, and the status entry of the one-call functions is written after U1 and U4 have merged.
- The stored scope of story:signed-url-token-copies named
  `examples/realtime-voice/examples/compare.rs`, the collision with story:realtime-voice-demo. The
  scoper found the story does not need that file (`adapters/inference-elevenlabs/src/agent_duplex.rs:332`
  keeps its signature); the entry is removed.
- Outside this wave: story:test-run-step-under-budget (approval-record:wave-2026-11-24-test-budget,
  its own branch and pull request, not active on this branch, so the verb does not print it) edits
  `Taskfile.yml` and `AGENTS.md`, which U4 also edits, in other sections. That pull request merges
  first; `main` is then merged into this wave's branch before its gate.
- story:audio-device-adapter, story:ess-defect-fixes and story:realtime-voice-demo are printed
  because they are active; they are not in this wave.
- story:speak-text-file-to-audio-file and story:sentence-clips-with-durations are active since
  approval-record:gap-stories-accepted-2026-11-24 and unassessed; they are scoped before the wave
  after this one is proposed.

## Pre-flight

- Free disk on the workstation: 47 G (`df -h /`, 2026-11-24 13:49 CET). One unit's build directory
  measured 1.9 G after `task test-build` (the wave 1 unit). Units gate package-scoped and build
  only their own crates.
- The shared build pool: 32 of 40 slots free at the same time.
- N is 4, the default.

## Commits this approval authorises

Unit commits on the four branches above, their merges into `wave/2026-11-24-dub-voice`, the store
commits of this wave (this record, the moves, scope, evidence, the three drafts, the reordered
trajectory page), the `CHANGELOG.md` entries, the push of the branches, one pull request into
`main` and its merge by the operator when the checks are green. No tag, no version bump, no
release.

## Integration

Branch `wave/2026-11-24-dub-voice`, from `origin/main` at `21cab7876`.

| unit | branch | worktree id | build directory | scratch | stage |
|---|---|---|---|---|---|
| U1 | `impl/sdk-one-call` | `inference-w2-u1-one-call` | `target/` in the unit's worktree | `wave2/u1` under the session scratch | implementor running |
| U2 | `impl/synthesiser-drop-inside-runtime` | `inference-w2-u2-drop` | `target/` in the unit's worktree | `wave2/u2` under the session scratch | waits for U3's merge |
| U3 | `impl/signed-url-token-copies` | `inference-w2-u3-token` | `target/` in the unit's worktree | `wave2/u3` under the session scratch | implementor running |
| U4 | `impl/local-tts-adapter` | `inference-w2-u4-local-tts` | `target/` in the unit's worktree | `wave2/u4` under the session scratch | scope written, implementor dispatched |
