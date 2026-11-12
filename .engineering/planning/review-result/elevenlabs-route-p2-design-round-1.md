---
format: aep.planning-md/3
id: review-result:elevenlabs-route-p2-design-round-1
kind: review-result
status: active
title: Plan critic, design, round 1, on wave P2 of epic:elevenlabs-route
relations:
- reviews: epic:elevenlabs-route
revision: 1
---
needs-revision

- story:async-speech-handles — its Planned scope says it depends on story:elevenlabs-agent-session-overrides, an archived story outside this wave that no edge records, so the sentence claims an ordering that can never be met; drop that story from the sentence, since the P1 edges already cover the ElevenLabs bindings — .engineering/planning/story/async-speech-handles.md:110, `aep plan artifact graph`
- task:release-elevenlabs-route-p2 — its Planned scope and its acceptance (merge-base ancestry) both require story:elevenlabs-agent-session-overrides, which is archived, out of this wave and has no edge from this task, so the acceptance cannot be met; the body must name the five stories and tasks it actually depends on (async handles, status page, flake, compare, latency run) and drop the archived story — .engineering/planning/task/release-elevenlabs-route-p2.md:27, .engineering/planning/task/release-elevenlabs-route-p2.md:37

What I read: 6 artifacts in the set, plus the P1 items, the archived overrides story and the approval record. I ran `aep plan artifact show` on each, then `relations`, `graph` and `validate`. `validate` printed `valid`; the 18 review-outcome reminders are pre-existing and not mine.

What I could not establish:
- Edges walked: about 95 edges in the whole graph, including P1, the archived story and the approval and review records. I did not stop at the set's ids. The `depends_on` edges form no cycle and the set is not a queue. The release depends on all five in parallel, and the status page depends on release-p1.
- Acceptance reads traced: 8 to a producer. 7 are recorded by an edge: the status page to release-p1, compare to `talk-picks-elevenlabs`, the latency run to compare, the release to its five dependencies, and async to the P1 stories. 1 is not: the release's read of the archived overrides story, which is the second finding above.
- Compare reads the ElevenLabs recogniser with no direct edge to story:elevenlabs-realtime-stt-adapter. That story is implemented and the order holds through story:talk-picks-elevenlabs, so I did not file it.
- Out of my lane, for parallel-safety:
  - Async, compare and the flake story edit `CHANGELOG.md` and `Cargo.toml` with no edge to task:release-elevenlabs-route-p1, whose Shared files line says the P2 stories depend on it.
  - Compare and the latency run both edit `website/docs/guides/compare-voice-providers.md`. An edge covers that one.
  - Compare and the flake story both write under `examples/realtime-voice/tests`.
- Out of my lane, for acceptance: the release requires a `Breaking` heading naming the async handle change. The async design keeps the blocking forms and adds a trait default, so that heading looks unwarranted.
- Unease I did not file: compare combines the Ollama live check on `inference-llama-server` with the compare flags. They share no file apart from the docs. I read this as the drafter's call, not a broken shape.

```findings
- file: .engineering/planning/story/async-speech-handles.md
  line: 110
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its Planned scope says it depends on story:elevenlabs-agent-session-overrides, an archived story outside this wave that no edge records, so the sentence claims an ordering that can never be met; drop that story from the sentence, since the P1 edges already cover the ElevenLabs bindings"
- file: .engineering/planning/task/release-elevenlabs-route-p2.md
  line: 37
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its Planned scope and its acceptance (merge-base ancestry) both require story:elevenlabs-agent-session-overrides, which is archived, out of this wave and has no edge from this task, so the acceptance cannot be met; the body must name the five stories and tasks it actually depends on (async handles, status page, flake, compare, latency run) and drop the archived story"
```