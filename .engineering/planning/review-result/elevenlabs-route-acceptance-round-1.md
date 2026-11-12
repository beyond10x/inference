---
format: aep.planning-md/3
id: review-result:elevenlabs-route-acceptance-round-1
kind: review-result
status: active
title: Acceptance critic, ElevenLabs route, round 1
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

task:release-elevenlabs-route-p1 — the acceptance never says the tag carries the two stories, `#39` or the `Breaking` changelog entry, so "points at a `main` commit whose … runs are green" holds for any green commit: "The tag exists on `origin`, points at a `main` commit whose `inference / task check` and `Security and privacy` runs are green, and its GitHub release has scanned notes." — .engineering/planning/task/release-elevenlabs-route-p1.md:33
task:release-elevenlabs-route-p1 — the acceptance joins two independent outcomes, the tag and the breaking-changes page, so one can pass while the other fails — .engineering/planning/task/release-elevenlabs-route-p1.md:35
story:elevenlabs-agent-session-overrides — the usage half of the scope has no check, and "unchanged except for what usage adds" names no observable usage outcome: "The claims record and `agent_capabilities` are unchanged except for what usage adds." — .engineering/planning/story/elevenlabs-agent-session-overrides.md:41
story:duplex-session-config-additions — "A test per addition on the OpenAI Realtime fixture" assumes OpenAI Realtime supports all five additions, while the scope says it implements only "those its protocol has" and the acceptance does not name which — .engineering/planning/story/duplex-session-config-additions.md:34
story:status-page-voice-adapters — "the version whose `CHANGELOG.md` section introduced it" cannot be checked for ElevenLabs STT, because that section exists only after task:release-elevenlabs-route-p1 and the story does not wait for that task — .engineering/planning/story/status-page-voice-adapters.md:29

What I read: 7 of 7 ids, in full, with `aep plan artifact show <id>` for each, plus `aep plan artifact kinds`, `epic:elevenlabs-route` and the tree checks below. In the tree, `git grep`/`grep` confirmed that `adapters/inference-elevenlabs/src/lib.rs:26-31`, `agent_duplex.rs:23-24` and the `0.27.0` tag exist. They also showed that `website/docs/reference/adapters.md` has no "Since" column today, so the status-page story does describe a real transition.

What I could not establish:
- I did not run `aep plan artifact lifecycle` for story or task. No finding depends on a status name.
- The two STT and TTS stories, the async-handles story and the other acceptances passed. Their fixture tests fail before the work, and each check can be run.
- Out of my lane, not counted in the verdict: the status-page story depends on the STT story but not on the release task, which is a design or ordering question. The async-handles scope also lists items with no acceptance, namely every adapter providing the async form, pipeline nodes using it, and the `futures-core`-only dependency limit. That is a scope-coverage question.
- The stories use multi-bullet acceptances. I treated a bullet list that must all pass as one checklist and did not flag it, except where the bullets are independent outcomes (the release task).

```findings
- file: .engineering/planning/task/release-elevenlabs-route-p1.md
  line: 33
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance never says the tag carries the two stories, #39 or the Breaking changelog entry, so 'points at a main commit whose inference / task check and Security and privacy runs are green' holds for any green commit"
- file: .engineering/planning/task/release-elevenlabs-route-p1.md
  line: 35
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance joins two independent outcomes, the tag and the breaking-changes page, so one can pass while the other fails"
- file: .engineering/planning/story/elevenlabs-agent-session-overrides.md
  line: 41
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the usage half of the scope has no check, and 'unchanged except for what usage adds' names no observable usage outcome"
- file: .engineering/planning/story/duplex-session-config-additions.md
  line: 34
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "'A test per addition on the OpenAI Realtime fixture' assumes OpenAI Realtime supports all five additions, while the scope says it implements only 'those its protocol has' and the acceptance does not name which"
- file: .engineering/planning/story/status-page-voice-adapters.md
  line: 29
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "'the version whose CHANGELOG.md section introduced it' cannot be checked for ElevenLabs STT, because that section exists only after task:release-elevenlabs-route-p1 and the story does not wait for that task"
```
