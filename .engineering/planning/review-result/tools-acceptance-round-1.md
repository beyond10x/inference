---
format: aep.planning-md/3
id: review-result:tools-acceptance-round-1
kind: review-result
status: active
title: aep:plan-critic-acceptance on epic:tools-and-turns, round 1
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
needs-revision

tool-call-lifecycle — the acceptance joins two independently checkable outcomes with "and": the conformance run over every `inference.tools` scenario, and the separate `cargo test` of §23.1 test 5, so one can pass while the other fails — `.engineering/planning/story/tool-call-lifecycle.md:86`
endpoint-controller — the acceptance asserts "a span left Hypothesis in a completed turn is unchanged", but the only fixture the body describes is generated audio (tone, silence, tone, silence) into a recording sink, so no span exists to observe; the acceptance names no span source — `.engineering/planning/story/endpoint-controller.md:53`
tool-call-interruption — for the Started case the acceptance accepts "Completed or Failed without reinjection", which reads the same whether or not the executor ever received the cancellation signal, so the cancellation the story exists to verify is not observable; the acceptance names no record of the signal, such as a mock-log entry — `.engineering/planning/story/tool-call-interruption.md:52`
cascade-read-only-tool — the acceptance runs a real model, but the body says a run where the model proposes no call is "a model failure ... not a framework failure" and does not pin the model, the sampling or the pass rule, so the same check can pass on one run and fail on the next, and nobody can say whether the story is done; the acceptance also names no source for the "spoken question" audio — `.engineering/planning/story/cascade-read-only-tool.md:59`

**What I read:** 8 artifacts (the epic and all 7 stories), each whole, via `aep plan artifact show <id>`. I also ran `aep plan artifact kinds` and `aep plan artifact lifecycle story`. I checked the 12 existing scenario ids the stories cite against `.engineering/drafts/suite.json`. I read `ess/domains/tools.yaml`, the `Generation` entity in `ess/domains/stream.yaml`, and the suite's list of `inference.tools` and `Generation` scenario ids.

**Scenario ids absent from the suite:** all of them are in `tool-call-lifecycle`, which extends the specification, names the file (`ess/domains/tools.yaml`), the lifecycle states and one command per transition, and so is acceptable. The only id that other stories cite from that set is `inference.tools.ToolCall/state/Completed/refuses/inference.tools.FailToolCall` in `tool-call-interruption`. It is declared upstream by `tool-call-lifecycle`, and `tool-call-interruption` depends on it transitively through `tool-feedback-loop` and `tool-authority-and-executor-nodes`.

**What I could not establish:**
- Whether the synthesizer will generate exactly the `inference.tools.ToolCall/state/...` ids that `tool-call-lifecycle` lists. The existing `Turn` and `Generation` ids follow the same pattern, so I expect it will, but the suite can't confirm it until the specification is extended.
- How `tool-call-interruption` deterministically holds a call in Proposed (a pending confirmation) and in Authorized (before the executor starts). Only the Started hold ("mock executor holds a Started call until released") is described. This may be a gap in the test setup rather than in the acceptance, so I did not raise it as a finding.
- I treated acceptances that list several cases of one test fixture or pipeline (`tool-authority-and-executor-nodes`, `tool-safe-retries`, `tool-feedback-loop`, `tool-call-interruption`) as a single statement. A stricter reading would split them. This is a judgement call, not a finding.
- Out of my lane, not counted in the verdict: the `tool-safe-retries` scope puts an attempt count on the failure event and a metric, and the acceptance does not check it, so that is scope coverage. The same story needs a typed-failure field on `FailToolCall` that `tool-call-lifecycle` does not mention, so that is design coupling.

```findings
- file: .engineering/planning/story/tool-call-lifecycle.md
  line: 86
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance joins two independently checkable outcomes with 'and': the conformance run over every inference.tools scenario, and the separate cargo test of section 23.1 test 5, so one can pass while the other fails"
- file: .engineering/planning/story/endpoint-controller.md
  line: 53
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance asserts 'a span left Hypothesis in a completed turn is unchanged', but the only fixture the body describes is generated audio into a recording sink, so no span exists to observe; the acceptance names no span source"
- file: .engineering/planning/story/tool-call-interruption.md
  line: 52
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "for the Started case the acceptance accepts 'Completed or Failed without reinjection', which reads the same whether or not the executor ever received the cancellation signal, so the cancellation the story exists to verify is not observable; the acceptance names no record of the signal"
- file: .engineering/planning/story/cascade-read-only-tool.md
  line: 59
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance runs a real model, but the body says a run where the model proposes no call is a model failure and not a framework failure, and it pins no model, sampling or pass rule, so the same check can pass on one run and fail on the next; it also names no source for the spoken question audio"
```
