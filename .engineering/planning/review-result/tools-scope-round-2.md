---
format: aep.planning-md/3
id: review-result:tools-scope-round-2
kind: review-result
status: active
title: aep:plan-critic-scope on epic:tools-and-turns, round 2
relations:
- reviews: epic:tools-and-turns
- reviews: story:tool-call-lifecycle
- reviews: story:endpoint-controller
- reviews: story:tool-authority-and-executor-nodes
- reviews: story:tool-feedback-loop
- reviews: story:tool-safe-retries
- reviews: story:tool-call-interruption
- reviews: story:cascade-read-only-tool
revision: 1
---
approve

**What I read:** 8 artifacts, whole bodies, using `aep plan artifact show` on `epic:tools-and-turns`, the seven stories and `review-result:tools-scope-round-1`. I also ran `aep plan artifact graph`. I read these design lines in `docs/design/inference.md`: 81, 244, 625, 778, 999, 1002, 1046, 1129, 1141, 1145, 1161 and 1175. I grepped `story:pipeline-validation` for the check-5 handoff. I wrote the list of epic promises before opening any story.

**Coverage check:** I extracted 17 promises from the epic and traced all 17 to a story. I found no gap, no reach beyond the epic, no double claim and no silent narrowing.

| Promise | Claimed by |
|---|---|
| `inference-tools` split out, ToolCall lifecycle declared first in `tools.yaml` | `story:tool-call-lifecycle` |
| A-09, the `barred` and `proposed` scenarios, the lifecycle scenarios | `story:tool-call-lifecycle` |
| §23.1 test 5 | `story:tool-call-lifecycle` (Rust test); `story:cascade-read-only-tool` (no idempotent-write call in the audit) |
| Tool executor node | `story:tool-authority-and-executor-nodes` |
| Schema-validated, authorized proposals | `story:tool-authority-and-executor-nodes` |
| Feedback edge LLM to executor to LLM; bounded iteration rule; result reinjected with generation correlation | `story:tool-feedback-loop` |
| VAD node; turn-policy node that commits turns | `story:endpoint-controller` |
| Tool retries distinct from generation retries; idempotency keys; no exactly-once claim; §23.3 test 6 | `story:tool-safe-retries` |
| Cascade proposes from committed input, executes one read-only tool through the mock executor and speaks the result | `story:cascade-read-only-tool` |
| Side-effecting call on provisional input refused before it exists | `story:tool-call-lifecycle`; `story:tool-feedback-loop` (a barred proposal is dropped) |
| Cancellation and no rollback of a completed write (epic Context, 856 and 1175) | `story:tool-call-interruption` |

None of the three Out of scope items is breached: MCP or a real executor, a confirmation UI, tool execution while the user talks.

**Round-2 questions, answered:**
- **New nouns.** The feedback-bound noun in `ess/domains/graph.yaml` is the only one. It traces to the epic's "bounded iteration rule". `story:pipeline-validation` line 26 hands it to `story:tool-feedback-loop` by name, so it is not reach. This also resolves the round-1 out-of-lane note about a feedback bound that nothing declared.
- **New files.** These are all implementation homes for in-scope behaviour: the node files, `mock_tool_executor.rs`, `synthetic_audio.rs` and the test files. None is a new deliverable.
- **Demonstration section.** It is not reach. It serves the design's P3 exit condition at line 1161, "Local voice agent executes a read-only tool correctly", which the epic quotes.
- **Dropped promise.** None. The epic's Exit bullet says the cascade executes a read-only tool "through the mock executor". The acceptance runs the cascade declaration mock-bound, and the body states why (design 1002, model output varies).

**What I could not establish:**
- Whether a non-gating real-adapter demonstration satisfies the epic's exit condition (line 1161). The story states the choice openly, so I did not count it as a silent narrowing. It is the operator's call.
- `story:tool-safe-retries` covers §23.3 test 6 as executor-side redelivery and puts any transport out of scope. This is unchanged from round 1 and the narrowing is stated in the body.
- `story:cascade-read-only-tool` uses the commit-gate node and the local adapters of `epic:local-voice-cascade`, which has no stories yet. That is a design and parallel-safety question, outside my lane.

```findings
[]
```
