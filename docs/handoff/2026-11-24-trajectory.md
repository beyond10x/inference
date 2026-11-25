# Trajectory, 2026-11-24

State: release 0.33.0, `## [Unreleased]` empty when this page was first written at `745fe77b6`. The
planning store stays the authority; this page orders it. Reordered the same day by the consumer's
reported gaps.

## Vision

- A reusable Rust inference ecosystem: contracts first, adapters below them, pipelines of typed
  nodes, a runtime, an optional session facade (vision:inference).
- Voice agents are one kind of graph. Self-hosted and open source first.
- epic:sdk: a voice agent in ten lines, any stack a configuration change.

## Consumer

- No repository of the organization pins this workspace (Cargo manifests of the sibling checkouts,
  read 2026-11-24).
- Ran today: a screencast dub, 754 characters of narration for a 60 s recording. It needed a local
  voice, could not use this repository and ran a local synthesiser binary directly.
- Its gaps, in the order it met them:
  1. a local synthesiser behind the same contract as the hosted ones
     (story:local-tts-adapter-under-licence-policy);
  2. one call, text file in and audio file out, with the voice as configuration
     (story:speak-text-file-to-audio-file, on top of story:sdk-one-call);
  3. one clip per sentence with its duration (story:sentence-clips-with-durations);
  4. a check of the spoken form of numbers and names before synthesis
     (story:spoken-form-check-before-synthesis).

## Waves

1. story:test-run-step-under-budget: the test-run step ends within 120 s and the conformance suite
   is synthesised once per run. First by the test budget rule.
2. story:local-tts-adapter-under-licence-policy, story:sdk-one-call,
   story:signed-url-token-copies, then story:synthesiser-drop-inside-runtime
   (approval-record:wave-2026-11-24-dub-voice): a local voice speaks behind the TTS contract,
   synthesis is one call, the signed-URL token is not left in copies, dropping an idle synthesis
   does not block a worker.
3. story:speak-text-file-to-audio-file, story:sentence-clips-with-durations,
   story:spoken-form-check-before-synthesis (drafts; the operator accepts them): the dub runs on
   this repository from a text file to per-sentence audio files.
4. story:sdk-usage-cost, story:voice-cascade-recipe-tools,
   story:agent-turn-completed-names-its-turn: every handle reports usage and cost, the cascade
   recipe hands its tools to the model node, a completion closes only the turn it names.

After these: story:sdk-testing, then story:sdk-examples (after story:realtime-voice-demo).

## Not buildable in this repository now

- story:audio-device-adapter, story:realtime-voice-demo: active, no code planned; each waits on
  the owner's run with a microphone and a headset.
- story:ess-defect-fixes: active, blocked by a dependency.

## Release 0.34.0

Wave 1 changes nothing a consumer sees. 0.34.0 is cut when wave 2 has merged and `main` is green:
the local synthesiser, `speak` in one call with the `Voice`, `Chat` and `Transcriber` handles, the
two fixes, the `ess` pin moved.
