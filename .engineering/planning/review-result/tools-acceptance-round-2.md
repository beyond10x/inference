---
format: aep.planning-md/3
id: review-result:tools-acceptance-round-2
kind: review-result
status: active
title: aep:plan-critic-acceptance on epic:tools-and-turns, round 2
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

What I read: 8 artifacts (`epic:tools-and-turns`, the 7 stories) and `review-result:tools-acceptance-round-1`, each whole, via `aep plan artifact show <id>`. I also read the rubric and the acceptance procedure.

Round-1 findings, none still holding:
- `story:tool-call-lifecycle`: the §23.1 test 5 `cargo test -p inference-tools` moved out of the acceptance sentence into Check 3 (`.engineering/planning/story/tool-call-lifecycle.md:125`). The acceptance at line 100 now names a single conformance run.
- `story:endpoint-controller`: the unobservable span claim left the acceptance (lines 78-88). It is now Check 1 (line 92), where a mock recogniser scripted to emit one hypothesis and no commit names the span source and the observable ("leaves that span Hypothesis in `inference.stream.Spans`").
- `story:tool-call-interruption`: the acceptance now requires "a `cancelled` entry carrying its call_id in the mock executor log" (line 76). The mock writes that entry when the cancellation signal fires on a held call (lines 51-53), and the Started case is "read from that entry, not inferred from the call's final state". Check 1 covers the fail-on-release variant.
- `story:cascade-read-only-tool`: the acceptance (line 91) runs on the testkit mocks, with a scripted mock LLM and recogniser and the synthetic audio fixture as the question's source. The real-model run is a demonstration that "gates nothing" (lines 59-73), with model, sampling, prompt, audio, pass rule and failure rule pinned in a table.

I also checked each cited existing scenario id against `.engineering/drafts/suite.json`:

| Scenario id cited | In the suite |
|---|---|
| `Generation/transition/supersede/by/SupersedeGeneration/superseded` | yes |
| `OutputSegment/transition/play/by/AcknowledgePlayback/played` | yes |
| `Turn/transition/complete/by/CompleteTurn/completed` | yes |
| `OpenTurn/outcome/opened` | yes |
| `CompleteTurn/outcome/completed` | yes |
| `CompleteTurn/outcome/wrong-state` | yes |
| `ValidatePipeline/outcome/rejected` | yes |

The `inference.tools.ToolCall/...` ids are not in the suite yet. `story:tool-call-lifecycle` declares them as a specification extension and names the file (`ess/domains/tools.yaml`, lines 29-55), so they are allowed.

What I could not establish:
- Whether the synthesizer spells the extension ids as listed. `story:tool-call-lifecycle` Check 1 (line 120) already corrects the list from the re-synthesized suite. The suite's `state/<X>/refuses/<cmd>` ids cover every state a command does not start from (all three terminal Generation states appear), so the `Proposed`, `Started` and `Completed` refusal ids are plausible.
- How the tests hold a call in Proposed or Authorized in `story:tool-call-interruption`. The body describes both holds (lines 56-61), so this is a test-setup question, not an acceptance defect.
- Judgement call: I read `story:tool-call-lifecycle`'s "with the checks below holding" as a pointer to supporting checks, the same as in `story:endpoint-controller`, `story:tool-call-interruption` and `story:cascade-read-only-tool`. A stricter reader would say the lifecycle acceptance still spans the conformance run and the `inference-tools` test. I did not count it.
- Out of my lane, and not counted in the verdict:
  - Scope: `story:tool-safe-retries` puts an attempt count on the failure event and a metric, and no acceptance checks it.
  - Design coupling: `story:tool-safe-retries` needs a typed-failure field on `FailToolCall` that `story:tool-call-lifecycle` item 2 names only as "a typed failure".
  - Parallel safety: the mock-LLM scripting that `story:tool-call-interruption` needs (two calls in one round) differs from the "one call" mock that `story:tool-feedback-loop` scripts.

```findings
[]
```
