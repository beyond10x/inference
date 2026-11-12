---
format: aep.planning-md/3
id: review-result:elevenlabs-route-design-round-1
kind: review-result
status: active
title: Design critic, ElevenLabs route, round 1
relations:
- reviews: story:elevenlabs-realtime-stt-adapter
- reviews: story:elevenlabs-tts-provider-cancel
- reviews: task:release-elevenlabs-route-p1
- reviews: story:elevenlabs-agent-session-overrides
- reviews: story:async-speech-handles
- reviews: story:duplex-session-config-additions
- reviews: story:status-page-voice-adapters
revision: 1
---
needs-revision

- story:status-page-voice-adapters — its acceptance reads the `CHANGELOG.md` section that introduced the ElevenLabs STT adapter, and task:release-elevenlabs-route-p1 creates that section in its release commit. The graph has `depends_on` only to story:elevenlabs-realtime-stt-adapter, so waves can put the page beside the release. The fix is a `depends_on` edge to task:release-elevenlabs-route-p1. — .engineering/planning/story/status-page-voice-adapters.md:29-30, .engineering/planning/task/release-elevenlabs-route-p1.md:25-26
- story:elevenlabs-agent-session-overrides — it builds the override from the session's first message and language, but `SessionConfig` has no such fields. Nothing in the set adds a first message. Language arrives only as the transcription language that story:duplex-session-config-additions adds, and no edge or scope line says so. The body should name the contract addition for first message and either depend on story:duplex-session-config-additions or add the fields itself. — .engineering/planning/story/elevenlabs-agent-session-overrides.md:27, crates/inference-duplex/src/contract.rs:35-46
- story:elevenlabs-agent-session-overrides — its acceptance holds `agent_capabilities` unchanged except for usage, while story:duplex-session-config-additions makes every duplex binding, this one included, state new capability flags. The two stories then claim the same function (`agent_duplex.rs:141-155`), and no edge orders them. The Order section also puts overrides (P2) before config-additions (P3). The acceptance should be reworded or an edge added. — .engineering/planning/story/elevenlabs-agent-session-overrides.md:41, .engineering/planning/story/duplex-session-config-additions.md:28
- story:async-speech-handles — its scope gives the async form to "every adapter implementing the three contracts". That includes `ElevenLabsRecogniser`, which story:elevenlabs-realtime-stt-adapter creates, and the synthesiser and agent that story:elevenlabs-tts-provider-cancel and story:elevenlabs-agent-session-overrides rewrite. No `depends_on` edge records that order, and the P2 label in the epic is not an edge. Without edges, the new adapter is built against the blocking `Recogniser` and then reworked. Add `depends_on` edges to the three stories. — .engineering/planning/story/async-speech-handles.md:26, `aep plan artifact graph`

**What you read:** 8 artifacts (the 7 in the set plus epic:elevenlabs-route) via `aep plan artifact show`. I also ran `aep plan artifact relations`, `graph` and `validate`. I walked 31 edges in the elevenlabs-route subgraph and the neighbouring native-voice ones, and none of them cycles. `validate` reports no errors on this set. I traced 6 acceptance reads to a producer inside the set, and an edge records 1 of them (release to STT and TTS cancel). I also read `contracts.rs`, `agent_duplex.rs`, `status.rs` and AGENTS.md § release.

**What you could not establish:**
- Whether the shared `inference-tts` and `inference-stt` contract surfaces that story:elevenlabs-tts-provider-cancel and story:async-speech-handles both edit can run in parallel. That is the parallel-safety critic's lane and does not affect this verdict.
- Whether the acceptance lines are checkable, for example the "at least the injected delay" bound. That is the acceptance critic's lane.
- Whether the release task holds two things, the tag and the breaking-changes page. I left it alone because the page lists the new minor and so depends on the same CHANGELOG entry.

```findings
- file: .engineering/planning/story/status-page-voice-adapters.md
  line: 30
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its acceptance reads the CHANGELOG.md section that introduced the ElevenLabs STT adapter, which task:release-elevenlabs-route-p1 creates in its release commit, and no depends_on edge runs from it to that task"
- file: .engineering/planning/story/elevenlabs-agent-session-overrides.md
  line: 27
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it builds the override from the session's first message and language, which SessionConfig lacks; language arrives only as the transcription language story:duplex-session-config-additions adds, nothing adds a first message, and no edge or scope line records either"
- file: .engineering/planning/story/elevenlabs-agent-session-overrides.md
  line: 41
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its acceptance holds agent_capabilities unchanged except for usage while story:duplex-session-config-additions makes every duplex binding, this one included, state new capability flags, and no edge orders the two"
- file: .engineering/planning/story/async-speech-handles.md
  line: 26
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its scope gives the async form to every adapter, including the ElevenLabs recogniser, synthesiser and agent that the STT, TTS-cancel and agent-overrides stories create or rewrite, and no depends_on edge records that order"
```
