---
format: aep.planning-md/3
id: review-result:elevenlabs-route-parallel-round-2
kind: review-result
status: active
title: Parallel-safety critic, ElevenLabs route, round 2
relations:
- reviews: story:elevenlabs-realtime-stt-adapter
- reviews: story:elevenlabs-tts-provider-cancel
- reviews: task:release-elevenlabs-route-p1
- reviews: story:elevenlabs-agent-session-overrides
- reviews: story:async-speech-handles
- reviews: task:release-elevenlabs-route-p2
- reviews: story:duplex-session-config-additions
- reviews: story:status-page-voice-adapters
revision: 1
---
needs-revision

- story:elevenlabs-agent-session-overrides — its only order edge is `depends_on task:release-elevenlabs-route-p1`, and `aep plan artifact waves` follows story edges only, so it puts this story at wave 6 and story:elevenlabs-tts-provider-cancel at wave 10 although both claim `CHANGELOG.md` and p1 needs the tts story first; add `depends_on` edges from this story to story:elevenlabs-tts-provider-cancel and story:elevenlabs-realtime-stt-adapter recording the shared `CHANGELOG.md` and `records/`, or record a `scope` on the release tasks so the waves verb can place them — .engineering/planning/story/elevenlabs-agent-session-overrides.md:13, and `aep plan artifact waves` lines 65 (wave 6), 89 (wave 10) (cited).
- story:duplex-session-config-additions — its only edge is `depends_on task:release-elevenlabs-route-p2`, which `waves` does not follow, so it lands at wave 4, ahead of story:elevenlabs-agent-session-overrides (wave 6) and story:async-speech-handles (wave 13) with which it shares `crates/inference-duplex/src/contract.rs`, `adapters/inference-elevenlabs/src/agent_duplex.rs`, `ess/domains/duplex.yaml` and `crates/inference-duplex`; add story edges to both, or give the release tasks a scope — .engineering/planning/story/duplex-session-config-additions.md:13, and `aep plan artifact waves` lines 47, 65, 104 (cited; `waves` printed all three collisions).
- story:duplex-session-config-additions — its scope omits `adapters/inference-elevenlabs/tests`, where the ElevenLabs agent refusal tests land (existing `tests/agent_duplex.rs`), and `adapters/inference-openai/tests`, where the five OpenAI fixture tests land; stories stt, tts and overrides claim or write the first, so `waves` cannot report that collision, and the scope gains both test paths or the body says where the tests live — .engineering/planning/story/duplex-session-config-additions.md:63-65 (inferred, from `ls adapters/inference-elevenlabs/tests`).
- story:elevenlabs-agent-session-overrides — its scope omits `adapters/inference-elevenlabs/tests`, where its four fixture tests land (inferred, `tests/agent_duplex.rs`), and stories stt and tts both claim that directory (cited), so `waves` cannot report the collision; add the path to `scope` — .engineering/planning/story/elevenlabs-agent-session-overrides.md:74-79 (inferred).
- story:status-page-voice-adapters — its acceptance reads the `CHANGELOG.md` section that task:release-elevenlabs-route-p1 writes, and the recorded edge runs through a task, so `waves` places the story at wave 5, before story:elevenlabs-tts-provider-cancel (wave 10), which p1 waits on; a direct `depends_on story:elevenlabs-tts-provider-cancel` is the missing edge, or merge the two — .engineering/planning/story/status-page-voice-adapters.md:14, and `aep plan artifact waves` lines 60, 89 (cited).

What I read: 8 artifacts, with `aep plan artifact show` on each, `aep plan artifact graph` and `relations`, and `aep plan artifact waves` for stories and for tasks. I read the elevenlabs adapter tree, `AGENTS.md` and the collision list filtered to the six stories. Surfaces are cited for 8 of 8 (the two tasks cite `Cargo.toml`, `CHANGELOG.md` and `adapters/*/records/*.json` in their bodies). The tts story has 5 inferred scope entries, overrides 6, duplex-additions 6 and stt 1. None are unplaceable, though the two tasks carry no `scope:` field, so `waves --kind task` prints them as unassessed. I traced 11 acceptance reads to a producer in the set. 9 are recorded by a direct `depends_on` edge (p1 on stt and tts; p2 on overrides and async; async on stt, tts and overrides; status-page on p1 and stt). The other 2 are the duplex story's reads of the ElevenLabs agent fixture and `ess/domains/duplex.yaml`, and they are recorded only through task:p2.

What I could not establish:
- Whether `story:elevenlabs-multi-context-tts-connection` and `story:elevenlabs-streaming-tts-adapter` collide with this set. They are unassessed (no scope) and outside it, so I did not guess.
- The collisions of story:async-speech-handles with about 3,800 store stories outside the set. I did not assess them.
- Whether `scope: adapters` on story:async-speech-handles is acceptable. The body states it as deliberate ("worked alone"), so I name it for a person to decide and it does not set my verdict.
- Out of my lane: story:elevenlabs-tts-provider-cancel says it feeds the latency report of story:voice-topology-latency-per-step, which is outside the set. Whether that belongs in this story is for the design and scope critics.

```findings
- file: .engineering/planning/story/elevenlabs-agent-session-overrides.md
  line: 13
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its only order edge is depends_on task:release-elevenlabs-route-p1, which aep plan artifact waves does not follow, so waves puts it at wave 6 and story:elevenlabs-tts-provider-cancel at wave 10 although both claim CHANGELOG.md and p1 needs the tts story first; add depends_on edges to story:elevenlabs-tts-provider-cancel and story:elevenlabs-realtime-stt-adapter recording the shared CHANGELOG.md and records/, or record a scope on the release tasks"
- file: .engineering/planning/story/duplex-session-config-additions.md
  line: 13
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its only edge is depends_on task:release-elevenlabs-route-p2, which waves does not follow, so waves puts it at wave 4, ahead of story:elevenlabs-agent-session-overrides (wave 6) and story:async-speech-handles (wave 13) with which it shares contract.rs, agent_duplex.rs and ess/domains/duplex.yaml; add direct story edges to both, or give the release tasks a scope"
- file: .engineering/planning/story/duplex-session-config-additions.md
  line: 65
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "inferred surface: its scope omits adapters/inference-elevenlabs/tests and adapters/inference-openai/tests where its refusal and fixture tests land, so waves cannot report the collision on the ElevenLabs tests directory that stories stt, tts and overrides claim or write; add the test paths to scope"
- file: .engineering/planning/story/elevenlabs-agent-session-overrides.md
  line: 74
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "inferred surface: its scope omits adapters/inference-elevenlabs/tests where its four fixture tests land (tests/agent_duplex.rs), which stories stt and tts both claim, so waves cannot report the collision; add the path to scope"
- file: .engineering/planning/story/status-page-voice-adapters.md
  line: 14
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its acceptance reads the CHANGELOG.md section task:release-elevenlabs-route-p1 writes, and the edge runs through a task that waves skips, so waves puts it at wave 5 before story:elevenlabs-tts-provider-cancel (wave 10) which p1 waits on; add a direct depends_on story:elevenlabs-tts-provider-cancel or merge the two"
```
