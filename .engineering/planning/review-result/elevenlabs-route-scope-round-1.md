---
format: aep.planning-md/3
id: review-result:elevenlabs-route-scope-round-1
kind: review-result
status: active
title: Scope critic, ElevenLabs route, round 1
relations:
- reviews: story:elevenlabs-realtime-stt-adapter
- reviews: story:elevenlabs-tts-provider-cancel
- reviews: task:release-elevenlabs-route-p1
- reviews: story:elevenlabs-agent-session-overrides
- reviews: story:async-speech-handles
- reviews: story:duplex-session-config-additions
- reviews: story:status-page-voice-adapters
- reviews: epic:elevenlabs-route
revision: 1
---
needs-revision

- `epic:elevenlabs-route` — "binds … the agent from a tagged release … and drives the handles from async code" is promised, but the only release item, `task:release-elevenlabs-route-p1`, carries the two P1 stories and `#39`, so nothing ships the agent overrides, agent usage or async handles in a tag; a P2 release task is the natural claimant — `.engineering/planning/epic/elevenlabs-route.md:36`
- `story:duplex-session-config-additions` — the outcome (per-response instructions, item placement, transcription settings, speed and truncation on the OpenAI Realtime binding, so the consumer can move its own OpenAI voice loop onto `inference-duplex`) traces to no sentence in the Outcome, which is ElevenLabs STT, TTS and agent. It also serves a consumer-side migration, which sits close to the exclusion "any consumer-side adapter". The epic's Order names it in P3, but no promise backs it — `.engineering/planning/story/duplex-session-config-additions.md:17`
- `story:status-page-voice-adapters` — it claims Deepgram STT, Deepgram Voice Agent and Cartesia TTS "Since" versions, and no epic sentence asks for a status page. Only the ElevenLabs entries trace even loosely to the epic's ElevenLabs route. The epic's Order names it in P3, but its outcome is not in the Outcome section — `.engineering/planning/story/status-page-voice-adapters.md:23`
- `task:release-elevenlabs-route-p1` — the page listing "breaking changes per minor since `0.12.0`" traces to no promise in the epic, which asks only for a tagged release carrying the P1 work and `#39` — `.engineering/planning/task/release-elevenlabs-route-p1.md:28`

**What you read:** 8 artifacts, via `aep plan artifact show` on the epic and each of the 7 items, plus `aep plan artifact graph`. The graph shows no other item decomposing `epic:elevenlabs-route`. I also read `grep` hits in `adapters/inference-elevenlabs/src`, the `AGENTS.md` release rule, and `git log`.
- Promises extracted from the parent: 9. Traced to an item: 8. The one untraced promise is the tagged release for the P2 outcomes.
- The nine promises are: STT binding, TTS binding, agent binding, a tagged release, μ-law 8 kHz frames, provider-side cancel, time to first audio, usage per call, async handles.
- Existing code already covers these promises:
  - μ-law 8 kHz output in the TTS binding (`config.rs` `Ulaw8000`) and the agent binding (`agent.rs:331`).
  - TTS usage (`synthesiser.rs:532`).
  - `#39` is in `main` at `88f43b9c` and is claimed by the release task.

**What you could not establish:**
- Whether the drafter deliberately left out a P2 release; there was no drafter report to read. The epic's Order lists only the P1 release task.
- Whether the operator wants the two P3 stories kept despite the Outcome. I treated the Order as naming them, not as promising their outcomes.
- Out of my lane, and not setting the verdict: the P2 stories have no dependency on a release item, which is a design or parallel-safety question.

```findings
- file: .engineering/planning/epic/elevenlabs-route.md
  line: 36
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "\"binds ElevenLabs speech-to-text, text-to-speech and the agent from a tagged release ... drives the handles from async code\" is promised, but the only release item (task:release-elevenlabs-route-p1) carries the P1 stories and #39, so no item ships the agent overrides, agent usage or async handles in a tag; a P2 release task would take it"
- file: .engineering/planning/story/duplex-session-config-additions.md
  line: 17
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome (OpenAI Realtime duplex controls so the consumer can move its own OpenAI voice loop onto inference-duplex) traces to no sentence in the epic's Outcome, which is the ElevenLabs route, and sits close to the exclusion 'any consumer-side adapter'"
- file: .engineering/planning/story/status-page-voice-adapters.md
  line: 23
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the story claims status and reference entries for Deepgram STT, Deepgram Voice Agent and Cartesia TTS, and no epic sentence asks for a status page or for any non-ElevenLabs adapter"
- file: .engineering/planning/task/release-elevenlabs-route-p1.md
  line: 28
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the page listing breaking changes per minor since 0.12.0 traces to no promise in the epic, which asks only for a tagged release carrying the P1 work and #39"
```
