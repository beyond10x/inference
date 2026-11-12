---
format: aep.planning-md/3
id: review-result:elevenlabs-route-p2-scope-round-2
kind: review-result
status: active
title: Plan critic, scope, round 2, on wave P2 of epic:elevenlabs-route
relations:
- reviews: epic:elevenlabs-route
revision: 1
---
needs-revision

story:async-speech-handles — "the pipeline nodes use it where they wait today" (with the four `crates/inference-pipeline` entries in its scope) changes the pipeline crate's internal waiting, which no sentence of the epic asks for, since the epic promises only "async forms of the speech handles", and no acceptance or design decision covers it. Either drop the clause and the pipeline scope paths, or name the consumer outcome that needs it. — .engineering/planning/story/async-speech-handles.md:101 (epic: .engineering/planning/epic/elevenlabs-route.md:42)

**Round-1 finding.** The release task body now lists all five approved wave items, names no archived story, and its acceptance has merge-base checks for the four stories. See `.engineering/planning/task/release-elevenlabs-route-p2.md` Planned scope and Acceptance.

**What I read.**
- 8 artifacts: the epic first, the approval record, the 6 set items, and `review-result:elevenlabs-route-p2-scope-round-1`.
- Commands: `aep plan artifact show` on each, `aep plan artifact graph` filtered to this epic, and `grep -n` on the story files.

**Promise count.** I extracted 7 promises from the epic and traced 5 to an item.
- Covered:
  - STT/TTS, provider cancel and time to first audio: P1 stories outside the set.
  - Breaking-changes list since `0.12.0`: P1 outside the set.
  - Async handles: `story:async-speech-handles`.
  - Status page and adapter reference: `story:status-page-voice-adapters`.
  - "main holds changes no tag carries": `task:release-elevenlabs-route-p2`.
- Not claimed, by the operator's decision: the agent with prompt, first message, voice, language and usage (`epic:elevenlabs-route` line 41, archived), and the duplex session config (outcome 3, later).

**What I could not establish.**
- `story:compare-elevenlabs-cascade-and-ollama`, `story:realtime-voice-barge-in-stop-flake` and `task:voice-latency-comparison-run` have no sentence in the epic. `approval-record:wave-elevenlabs-route-p2` "Decision" names all three, so I did not count them as reach.
- `story:status-page-voice-adapters` acceptance gives a "Since" release to "every row, hosted and local", against the epic's "every hosted voice adapter". That is a column applied across a whole table, so I did not count it.
- Out of my lane, not setting the verdict:
  - `task:voice-latency-comparison-run` depends on `story:async-speech-handles`, and its body gives no reason.
  - The release acceptance's `merge-base` checks leave out the comparison task's guide commit.

```findings
- file: .engineering/planning/story/async-speech-handles.md
  line: 101
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the clause 'the pipeline nodes use it where they wait today' (with the crates/inference-pipeline scope paths) changes pipeline internals that no sentence of the epic asks for, since the epic promises only 'async forms of the speech handles', and no acceptance or design decision covers it; drop it or name the consumer outcome that needs it"
```