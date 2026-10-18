---
format: aep.planning-md/3
id: review-result:tools-scope-round-1
kind: review-result
status: active
title: aep:plan-critic-scope on epic:tools-and-turns, round 1
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

**What I read:** 9 artifacts, all whole bodies, via `aep plan artifact show` on `epic:tools-and-turns`, the seven stories, `epic:contracts-and-testkit`, `epic:graph-minimum` and `epic:local-voice-cascade`. I also ran `aep plan artifact graph`. I read `story:pipeline-validation`, `story:testkit-conformance-target` and `story:llm-contract` only by grep. I read `ess/domains/tools.yaml` and checked the design at `docs/design/inference.md` lines 75-85, 540-548, 848-868, 1000-1008, 1100-1180 and 1210-1218.

**Coverage check:** I extracted 15 promises from the epic and traced all 15 to a story. I found no gap, no reach beyond the epic, no double claim and no silent narrowing.

| Promise | Claimed by |
|---|---|
| `inference-tools` split out, ToolCall lifecycle declared first in `tools.yaml` | `story:tool-call-lifecycle` |
| Tool executor node | `story:tool-authority-and-executor-nodes` |
| Feedback edge with a bounded iteration rule | `story:tool-feedback-loop` |
| VAD node and turn-policy node that commit turns | `story:endpoint-controller` |
| Tool retries distinct from generation retries; idempotency keys for idempotent writes; no exactly-once claim | `story:tool-safe-retries` |
| A-09 and the barred/proposed/lifecycle scenarios | `story:tool-call-lifecycle` (schema validation in the authority story) |
| §23.1 test 5 | `story:tool-call-lifecycle` (Rust test); `story:cascade-read-only-tool` (no idempotent-write call in the audit) |
| §23.3 test 6 | `story:tool-safe-retries` (executor redelivery) |
| Cascade executes a read-only tool and speaks the result | `story:cascade-read-only-tool` |
| Result reinjected with generation correlation intact | `story:tool-feedback-loop` |
| Cancellation and no rollback of a completed write (from the epic's Context, not its Scope or Exit) | `story:tool-call-interruption` |

None of the three Out of scope items is breached (MCP or a real executor, a confirmation UI, tool execution while the user talks). Each story's own Out of scope repeats the exclusion.

**What I could not establish:**
- `story:tool-call-interruption` traces only to the epic's Context ("verify feedback, cancellation, and correlation" at line 1175, and 856), not to its Scope or Exit. I judged that traceable and not reach.
- `story:tool-safe-retries` covers §23.3 test 6 as executor redelivery and puts transport out of scope. The narrowing is stated, so I did not count it. Whether redelivery stands in for a reconnect is the operator's call.
- Out of my lane, not counted in the verdict:
  - `story:tool-feedback-loop` says to use a feedback bound if graph-minimum declared one. `story:pipeline-validation` (line 26) says none exists and that this epic declares it. A design or acceptance critic should check this.
  - The same story's acceptance needs `ValidatePipeline` to accept a bounded cycle, which `story:pipeline-validation` refuses today. That behaviour change is not stated.
  - `story:testkit-conformance-target` lists the 3 `inference.tools` ids as skipped. The lifecycle story does not say it removes those lines.

```findings
[]
```
