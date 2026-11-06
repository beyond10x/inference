---
format: aep.planning-md/3
id: review-result:adversary-w23-audio-chunk-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the AudioChunk envelope kind
relations:
- reviews: story:audio-envelope-kind-is-the-declared-event
revision: 1
---
unit: W23-audio-chunk, story:audio-envelope-kind-is-the-declared-event, covers 422f919 plus my commit 9d20bd4 (tree inference-0-23-0-audio-chunk)
verdict: nothing found (one coverage note, now closed by a passing case)
cases: executed 43→44, red 0
origin: introduced 0 / pre-existing 1 / undecided 0
wrote-outside-worktree: 5 paths (listed in part 6)
needs-coordinator: the third Acceptance line, the CHANGELOG `## [Unreleased]` Breaking line for `AUDIO_FRAME` → `AUDIO_CHUNK`, is the coordinator's to write and is not there yet. 422f919 was also not reviewed for the "a test asserts the TTS node emits AudioChunk" line beyond the two tests read below.

1. Diff: `git --no-pager diff --stat` is empty because the case is committed. 9d20bd4 adds one file only, `crates/inference-runtime/tests/adversary_w23_cascade_speech_kind.rs` (+214), a test path. I changed no implementation file.

2. Case added: `the_cascade_speech_node_stamps_every_frame_inference_audio_audio_chunk`. It runs the testkit cascade on the mocks and asserts each audio frame on `SPEECH_EGRESS` has kind `"inference.audio.AudioChunk"`, one per word of the reply. It has a 20 s deadline. Run alone (`cargo-run.sh test -p inference-runtime --test adversary_w23_cascade_speech_kind`): 1 passed, `rc=0 secs=4`. It is green, not red. It covers the third producer, `cascade/speech.rs`. No target that builds without `device` checked that producer's kind before: `voice_cascade_mocks` reads only the payloads, and the tests that stamp the old kind are in `local-voice-cascade`, which needs `device`. So a mutant putting the old kind back there would have passed every buildable target. I worked this out by reading and did not build a mutant copy.

3. Suite, run after the case existed:
- runtime: `adversary_w23_cascade_speech_kind` 1/0, `endpoint_controller` 3/0, `refusal_and_recorder_bounds` 3/0, `voice_cascade_mocks` 1/0 (`rc=0 secs=1`).
- testkit `--features tts,run`: `tts_usage` 5/0. `usage_on_cancel` ran 0 tests under those features, so I re-ran it with `tts,run,stt,llm`: 5/0 (`secs=7`).
- audio-device `--no-default-features --features pipeline`: `acknowledging` 17/0, `nodes` 8/0 (`secs=3`).
- `clippy -p inference-runtime --all-targets -D warnings`: rc=0 (`secs=6`). fmt is clean, and scan-text reported "1 file(s) carry no private rule".
- Cargo time was 31 s and wall time about 12 min. `target/` peaked at 1.6G and is now discarded.

4. Judgement:
- `ess/domains/audio.yaml:79`: the comment describes AudioChunk as "a frame of synthesized audio". The device source (`adapters/inference-audio-device/src/node.rs:44`) now stamps captured microphone frames AudioChunk, as the story's Outcome asks. The comment is now narrower than what the code does. Doc only.

5. Attacked and not broken:
- `rg 'inference\.audio\.AudioFrame' -g '!target'` matches only the struct, its doc lines, typed fields (audio.yaml, duplex.yaml:535/771/1218/1267), the inference-tts doc and historical CHANGELOG lines 107-108. Acceptance line 1 is met.
- No consumer in src matches, filters or routes on an audio kind string: runtime (drive, subgraph, record, queue), pipeline nodes, facade (live, session), the STT nodes (testkit, sherpa, whisper, deepgram), the duplex adapters and realtime-voice/compare. They match only `ControlKind`, `UsageRecorded::KIND` or the payload.
- Both producers take the kind from `event.name()` on their single frame path. The implementor's two tests assert the literal `"inference.audio.AudioChunk"`, so neither is self-referential.
- `talk.rs` and all lvc test helpers are on the new kind. `AUDIO_FRAME` is left nowhere: no website, README or rustdoc link to it.

6. Written outside the worktree, all under `<scratch>`: `adv1-case.log`, `adv1-suite.log`, `adv1-suite2.log`, `msg.VyeCl1`. The harness also wrote background-task output under `<scratch>`.

```findings
[{"file":"crates/inference-testkit/src/cascade/speech.rs","line":395,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"No test buildable without the device feature observed the kind the cascade speech node writes, so a regression to the old kind there survived every buildable target; closed by adversary_w23_cascade_speech_kind.rs (green)."},
 {"file":"ess/domains/audio.yaml","line":79,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The AudioChunk comment says synthesized audio only, while the device source now stamps captured microphone frames with the same event name."}]
```
