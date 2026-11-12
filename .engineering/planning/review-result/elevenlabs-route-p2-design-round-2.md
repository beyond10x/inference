---
format: aep.planning-md/3
id: review-result:elevenlabs-route-p2-design-round-2
kind: review-result
status: active
title: Plan critic, design, round 2, on wave P2 of epic:elevenlabs-route
relations:
- reviews: epic:elevenlabs-route
revision: 1
---
needs-revision

- task:voice-latency-comparison-run — its `depends_on story:async-speech-handles` edge has no reason in the body, and nothing it reads comes from that story. The Context and Acceptance read only `compare --clips`, `--runs-out` and `--cascade-stt` from story:compare-elevenlabs-cascade-and-ollama. Async keeps the blocking forms and says "examples/realtime-voice ... keep their blocking calls in this story". The edge puts the billed run behind the whole chain compare → flake → async. State the reason in the body (for example, to measure the tree that gets tagged), or drop the edge. — .engineering/planning/task/voice-latency-comparison-run.md:11, .engineering/planning/story/async-speech-handles.md:106, `aep plan artifact graph`

**What you read:** 6 artifacts, plus the round-1 design review-result. Commands: `aep plan artifact show` on each, `relations`, `graph` and `validate`. `validate` printed `valid`.

**Round 1:** both findings are resolved.
- story:async-speech-handles no longer cites the archived overrides story. Its Order paragraph names only the P1 stories, the flake story and the compare story.
- task:release-elevenlabs-route-p2 now depends on exactly five items (async, status page, flake, compare, latency run), and its acceptance names four merge commits.

**What I could not establish:**
- **Edges walked:** about 31 `depends_on` edges touching the set, filtered from the whole graph, including P1, the archived story, `story:duplex-session-config-additions` and the release records. I walked outside the set. There is no cycle, and the set is not a pure queue: the status page and the flake story both depend only on compare, so they can run side by side.
- **Acceptance reads traced:** about 14 to a producer in or just outside the set. All but one are recorded by an edge. The exception is compare's read of the ElevenLabs recogniser, which has no direct edge to story:elevenlabs-realtime-stt-adapter. That order holds through story:talk-picks-elevenlabs, and that story is implemented, so I did not file it. The latency run's edge to async is the only edge I traced that has no read behind it; it is the finding above.
- **Unease, not filed:** the async-to-flake edge. Its stated reason is that the examples build against the contract crates. The async design section says no implementor changes and the examples keep blocking calls, so the reason is weak. The two stories also share no file. It is a stated reason, so it is not a finding; the parallel-safety critic may judge it.
- **Out of my lane:** the status-page body does not state why it depends on compare. Compare's body does (adapters.rs ownership and the test named at `status.rs:328`).
- **Out of my lane:** the release summary says "after the two P2 stories", but the body lists five dependencies. That is wording.

```findings
- file: .engineering/planning/task/voice-latency-comparison-run.md
  line: 11
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its depends_on story:async-speech-handles edge has no reason in the body and nothing it reads comes from that story, so it serialises the billed run behind the compare, flake, async chain; state the reason in the body or drop the edge"
```