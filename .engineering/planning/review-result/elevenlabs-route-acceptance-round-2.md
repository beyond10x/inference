---
format: aep.planning-md/3
id: review-result:elevenlabs-route-acceptance-round-2
kind: review-result
status: active
title: Acceptance critic, ElevenLabs route, round 2
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

story:status-page-voice-adapters — the acceptance defines "Since" as "the first `CHANGELOG.md` section naming its crate or binding", which contradicts the body's "section that introduced it". It makes the check give wrong values for sibling adapters in one crate: `inference-elevenlabs` is first named in 0.16.0, so TTS, agent (introduced in 0.17.0) and the new STT all come out as 0.16.0, and Deepgram STT and Voice Agent share a crate the same way. The acceptance should name the section by adapter or binding, for example the first section naming `ElevenLabsRecogniser`, `ElevenLabsAgent` or `ElevenLabsSynthesiser`. — .engineering/planning/story/status-page-voice-adapters.md:44-45 (`CHANGELOG.md:317,484,523` name `inference-elevenlabs` in 0.22.0, 0.17.0 and 0.16.0)

story:elevenlabs-agent-session-overrides — the usage bullet is "either a documented usage message … yields a usage record, or, where the protocol documents none, … yields no usage record". It does not say which outcome is required, so a binding that records nothing passes by declaring the protocol silent, and the check reduces to the implementor's reading of the docs. The body should record which branch holds, or say that the implementor reads the protocol and cites the documentation page and date in the claims record. — .engineering/planning/story/elevenlabs-agent-session-overrides.md:79-81

story:elevenlabs-agent-session-overrides — the third bullet, "a session that sets a first message on a binding that cannot honour it is refused at open as `Unsupported`", names no binding. This story makes the ElevenLabs agent honour a first message, so the test has no stated subject, and the "every duplex binding states which it takes" scope item has no check at all. The acceptance should name the binding, such as the testkit mock or the Deepgram agent, and add a check that each binding states its flags. — .engineering/planning/story/elevenlabs-agent-session-overrides.md:77-78

task:release-elevenlabs-route-p1 — the acceptance says the changelog section has "the 'Breaking since 0.12.0' list" but gives no criterion for it. The scope requires one line per minor from 0.13.0 to the new one that had a `Breaking` section, each linking that section, and a one-line list would pass the check as written. The acceptance should say the list has exactly one linked line per such minor, checkable against `grep -n '^### Breaking' CHANGELOG.md`. — .engineering/planning/task/release-elevenlabs-route-p1.md:40-42 (`CHANGELOG.md:5,52,132,212,246` show the `Breaking` sections the list must cover)

**What I read:** all 8 ids, in full, with `aep plan artifact show <id>` (8 of 8). I also read `aep plan artifact kinds`, `lifecycle story` and `lifecycle task`, and `epic:elevenlabs-route` for context. In the tree I checked `git tag`, `cargo tree` for tokio, `Taskfile.yml` `docs-check`, `CHANGELOG.md`, `website/docs/reference/adapters.md` and the cited line anchors in the STT and TTS contract crates.

**What I could not establish:**
- The bulleted acceptance lists on stories follow the AGENTS.md rule that "a story's acceptance is a set of named conformance scenarios". I did not treat them as the multiple-statement defect.
- I did not flag the task acceptances as multiple statements either. Each is one release with one set of checks.
- `story:duplex-session-config-additions` requires a refusal test for all five additions on the ElevenLabs agent. The body says a binding states its flags false "unless its protocol has the control". I could not tell whether the agent has one of the five, such as speed, so some of those refusal tests may be unwritable. I left this alone because it needs the protocol read.
- `story:async-speech-handles` scope says every adapter and the pipeline nodes get the async form. The acceptance exercises only the three ElevenLabs bindings and the backend-neutral checks. I judged that a coverage question for the scope critic.
- The `cargo tree … no tokio` bullet already holds today (`grep -c tokio` printed 0), so it guards a constraint and does not show a transition. The other bullets of that story carry the transition.
- Out of my lane: `story:elevenlabs-agent-session-overrides` context says eleven `ResponseUsageRecorded` in `adapters/inference-openai/src/duplex.rs`. `git grep -o` finds one there. That is a factual error in the context, not in the acceptance.

```findings
- file: .engineering/planning/story/status-page-voice-adapters.md
  line: 44
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance defines Since as 'the first CHANGELOG.md section naming its crate or binding', which contradicts the body's 'section that introduced it' and yields 0.16.0 for ElevenLabs STT, TTS and agent alike, since inference-elevenlabs is first named there; it should name the section by adapter or binding"
- file: .engineering/planning/story/elevenlabs-agent-session-overrides.md
  line: 79
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the usage bullet is 'either a documented usage message ... yields a usage record, or, where the protocol documents none, ... yields no usage record', so a binding that records nothing passes by declaring the protocol silent; it does not say which outcome is required"
- file: .engineering/planning/story/elevenlabs-agent-session-overrides.md
  line: 77
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance refuses a first message 'on a binding that cannot honour it' but names no binding, since the ElevenLabs agent now honours it, and no check shows that every duplex binding states which of the two fields it takes"
- file: .engineering/planning/task/release-elevenlabs-route-p1.md
  line: 40
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance requires 'the \"Breaking since 0.12.0\" list' with no criterion, so a one-line list passes; it should say one linked line per minor from 0.13.0 that has a Breaking section"
```
