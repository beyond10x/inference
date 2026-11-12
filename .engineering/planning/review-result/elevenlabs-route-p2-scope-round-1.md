---
format: aep.planning-md/3
id: review-result:elevenlabs-route-p2-scope-round-1
kind: review-result
status: active
title: Plan critic, scope, round 1, on wave P2 of epic:elevenlabs-route
relations:
- reviews: epic:elevenlabs-route
revision: 1
---
needs-revision

task:release-elevenlabs-route-p2 — the body still makes the archived story:elevenlabs-agent-session-overrides part of release 0.29.0 and does not name the approved comparison, status page or flake fix, so its acceptance can never be met and it checks none of the approved items — .engineering/planning/task/release-elevenlabs-route-p2.md:22,27,37 (against approval-record:wave-elevenlabs-route-p2, "Decision")

What I read: 7 artifacts (epic, approval record, 6 set items, plus the archived agent-overrides story for the stale-reference check). Commands: `aep plan artifact show` on each, `aep plan artifact graph`, `grep -n` on the epic and release files. The epic was read first.

Promise count: I extracted 6 promises from the epic and traced 4 to an item:
- Promise 1, the STT, TTS, cancel and breaking-change release, is covered by the P1 items outside the set.
- Promise 2b, async handles, is covered by story:async-speech-handles.
- Promise 4, status page and adapter reference, is covered by story:status-page-voice-adapters.
- Promise 5, the release that carries what `main` holds, is covered by task:release-elevenlabs-route-p2.
- Promise 2a, the agent with prompt, first message, voice, language and usage (epic line 41), has no item. The operator archived it, so this is not a gap.
- Promise 3, duplex session config (epic outcome 3), has no item. The operator excluded it, so this is not a gap.

Unease, not findings:
- **Scope of the three items beyond the epic:** story:realtime-voice-barge-in-stop-flake, story:compare-elevenlabs-cascade-and-ollama and task:voice-latency-comparison-run have no sentence in the epic behind them. The approval record names all three, so I did not count them as reach beyond the parent.
- **The epic's own text:** it still promises the agent work (lines 41 and 55) and still lists agent-overrides in P2. That is the parent's text, not the set's, and the operator's decision covers the omission.
- **Local rows on the status page:** story:status-page-voice-adapters acceptance says "every row, hosted and local" gets a Since value, while the epic and its planned scope say hosted. Worth a glance, but a column across a whole table may justify it.
- **Out of my lane, for the other critics:**
  - story:async-speech-handles still names the archived story as a dependency in its body, but not in its relations.
  - Its body also names story:duplex-session-config-additions, which is out of the wave.

```findings
- file: .engineering/planning/task/release-elevenlabs-route-p2.md
  line: 22
  category: scope
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the body still makes the archived story:elevenlabs-agent-session-overrides part of release 0.29.0 (lines 22, 27, 37), so its ancestor acceptance can never be met, and it does not name the approved status page, flake fix and cascade comparison that approval-record:wave-elevenlabs-route-p2 puts in the release"
```