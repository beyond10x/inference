# Trajectory, 2026-11-24

State: release 0.33.0, `main` at `745fe77b6`, `## [Unreleased]` empty. The planning store stays the
authority; this page orders it.

## Vision

- A reusable Rust inference ecosystem: contracts first, adapters below them, pipelines of typed
  nodes, a runtime, an optional session facade (vision:inference).
- Voice agents are one kind of graph. Self-hosted and open source first.
- epic:sdk: a voice agent in ten lines, any stack a configuration change.

## Consumer

- No repository of the organization pins this workspace (Cargo manifests of the sibling checkouts,
  read 2026-11-24).
- Running today: a screencast dub through speech synthesis (`Recipe::speak`, the hosted TTS
  adapters).
- Its first gap is not reported yet. Read from the plan: there is no one-call `speak` and no
  `Voice` handle (story:sdk-one-call, draft). A reported gap replaces this reading.

## Waves

1. story:test-run-step-under-budget: the test-run step ends within 120 s and the conformance suite
   is synthesised once per run. First by the test budget rule.
2. story:sdk-one-call, story:synthesiser-drop-inside-runtime, story:signed-url-token-copies and
   the dub's reported gaps: synthesis is one call, dropping an idle synthesis does not block a
   worker, the signed-URL token is not left in copies.
3. story:sdk-usage-cost, story:voice-cascade-recipe-tools,
   story:agent-turn-completed-names-its-turn: every handle reports usage and cost, the cascade
   recipe hands its tools to the model node, a completion closes only the turn it names.

After these: story:sdk-testing, then story:sdk-examples (after story:realtime-voice-demo).

## Not buildable in this repository now

- story:audio-device-adapter, story:realtime-voice-demo: active, no code planned; each waits on
  the owner's run with a microphone and a headset.
- story:ess-defect-fixes: active, blocked by a dependency.
- story:local-tts-adapter-under-licence-policy: waits on the licence policy.

## Release 0.34.0

Wave 1 changes nothing a consumer sees. 0.34.0 is cut when wave 2 has merged and `main` is green:
`speak` in one call, the `Voice` handle, the two fixes, the `ess` pin moved.
