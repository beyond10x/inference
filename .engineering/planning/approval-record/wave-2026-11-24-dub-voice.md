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
revision: 7
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

`aep plan artifact waves --kind story --status active` on the wave's branch, run again after
story:ticker-tests-hold-on-a-loaded-host was accepted
(approval-record:ticker-tests-accepted-2026-11-24), printed:

```
wave 1
  story:audio-device-adapter
  story:ess-defect-fixes
  story:local-tts-adapter-under-licence-policy (inferred)
  story:sdk-one-call (inferred)
  story:signed-url-token-copies (inferred)
  story:ticker-tests-hold-on-a-loaded-host
wave 2
  story:realtime-voice-demo (inferred)
  story:synthesiser-drop-inside-runtime (inferred)
collision: story:signed-url-token-copies story:synthesiser-drop-inside-runtime CHANGELOG.md (inferred)
collision: story:signed-url-token-copies story:synthesiser-drop-inside-runtime adapters/inference-elevenlabs/src/lib.rs
collision: story:synthesiser-drop-inside-runtime story:ticker-tests-hold-on-a-loaded-host adapters/inference-elevenlabs/tests/async_handles.rs (inferred)
unassessed: story:sentence-clips-with-durations
unassessed: story:speak-text-file-to-audio-file
2 wave(s), 3 collision(s), 2 unassessed
```

- U2 and U3 collide on `adapters/inference-elevenlabs/src/lib.rs` (U2 lines 41 and 52-55, U3 lines
  71-88) and on `CHANGELOG.md`. They do not run at once in that file: U3 runs first, and the
  integration branch is merged into U2's branch after U3 has merged, before U2 touches `src`.
- story:ticker-tests-hold-on-a-loaded-host and story:synthesiser-drop-inside-runtime collide on
  `adapters/inference-elevenlabs/tests/async_handles.rs`. One unit, U2, builds both, one after the
  other: the ticker story first (it edits only that test file, so it runs beside U3), then the drop
  story.
- U4 is placed beside U1 and U3.
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

Branch `wave/2026-11-24-dub-voice`, from `origin/main` at `21cab7876`; `origin/main` at
`fb7192ac8` merged in on 2026-11-25 (`c6a4e9a91`).

State on 2026-11-25. The wave's pull request carries U2 and U3. U1 and U4 are not in it: each
waits for a ruling of the owner and follows as a pull request of its own.

| unit | stories | branch | stage |
|---|---|---|---|
| U1 | story:sdk-one-call | `impl/sdk-one-call` | not merged. At `2fb2f9ae2` one case is red: the bound of the tool loop. The specification's sentence on that bound has to be amended before the code, in a commit of its own, and that edit waits for the owner's yes. The `Chat`, `Transcriber` and `Voice` handles are therefore not in the wave's pull request. |
| U2 | story:ticker-tests-hold-on-a-loaded-host, story:synthesiser-drop-inside-runtime | `impl/synthesiser-drop-inside-runtime` | merged at `e4d23dcdd` (`894e900e6`). One adversary pass on the ticker story, two on the drop story, each recorded with its outcome. |
| U3 | story:signed-url-token-copies | `impl/signed-url-token-copies` | merged at `ba1870be0` (`e04621e9f`). Two adversary passes, each recorded with its outcome. |
| U4 | story:local-tts-adapter-under-licence-policy | `impl/local-tts-adapter` | not merged. Two adversary passes are recorded; the correction of the second runs in the unit's branch. story:sherpa-onnx-linked-libraries-licences-read found libraries in the linked archives whose licences are outside the list of `AGENTS.md`; whether the adapter ships, and how, is the owner's ruling. |

No release is tagged from this wave before both rulings.
