---
format: aep.planning-md/3
id: review-result:contracts-scope-round-2
kind: review-result
status: active
title: aep:plan-critic-scope on epic:contracts-and-testkit, round 2
relations:
- reviews: epic:contracts-and-testkit
- reviews: story:workspace-skeleton
- reviews: story:testkit-conformance-target
- reviews: story:outbound-text-generations-and-segments
- reviews: story:inbound-text-spans-and-envelope
- reviews: story:audio-vocabulary
- reviews: story:audio-and-stt-contract
- reviews: story:llm-contract
- reviews: story:tts-contract
revision: 1
---
approve

What I read: 9 artifacts with `aep plan artifact show`: the epic, the round-1 verdict and the 8 stories. I also ran `aep plan artifact graph` and read the critic rubric. A mechanical check compared the 60 scenario ids listed in the two stream stories with the 60 `inference.stream` ids in `.engineering/drafts/suite.json`. The sets are identical, with no overlap and no gap.

Promises I extracted from the revised epic: 16. I traced all 16 to a story.
- Outcome (2): the consumer writes a mock against one contract crate and the §23.1 tests pass; the testkit answers the model and stream scenarios.
- `inference-core` (6): identifiers, capability and related enums, envelope with timebase and `kind`, `Generation`, typed errors, control events as narrowed.
- `inference-text` (1).
- `inference-audio` (1, as narrowed).
- The three domain traits (1).
- `inference-testkit` (1).
- Exit (3): the `report` route plus the named scenarios, §23.1 tests 1, 2, 3 and 6, and A-01.
- The no-concrete-implementation constraint (1).

How the three round-1 findings stand:
- **Timebase in `inference-audio` (blocker):** fixed. The epic now says the promise is "Narrowed from 'timebases'" and gives the reason (`.engineering/planning/epic/contracts-and-testkit.md:54-58`). `story:audio-vocabulary` claims frames, formats and sample clock position, and its Out of scope sends the enum to `story:inbound-text-spans-and-envelope`. The inbound story's Context (the paragraph "Two items of the epic's `inference-core` scope are claimed here") claims it.
- **Control events (warning):** fixed. The epic now records the narrowing and the reason (`.engineering/planning/epic/contracts-and-testkit.md:46-50`), and the inbound story claims it in the same paragraph. The epic's count of six `inference.stream` errors matches 2 in the outbound story plus 4 in the inbound story.
- **`ess verify conform report` missing from the testkit story (warning):** fixed. The epic's Exit now names the route and says why `run` is not used (`.engineering/planning/epic/contracts-and-testkit.md:46-52`). `story:testkit-conformance-target` Context and Step 2 write `results.json`, run `report --suite --results --report-out`, and assert on the resulting report. Its Acceptance repeats that.

Reach beyond the epic: none found. The new `inference.audio` domain, with its entries in `ess/system.yaml` and `ess/ess-inputs.yaml`, is named in the epic's Context as arriving with this epic. Each story's Out of scope keeps clear of the epic's exclusions: nodes, ports, pipelines, the runtime, duplex, tools, the facade and real backends.

What I could not establish, none of which sets the verdict:
- **Stub modules for other domains.** `story:testkit-conformance-target` creates empty store and target modules for `policy`, `graph`, `tools` and `session`, and `ess/SKIPPED.md` sections naming the sibling epics. This goes past the epic's "two domains above". I did not count it because `report` refuses a scenario with no result, so every suite scenario needs a status. Whether the empty scaffolding is acceptable is a parallel-safety and design question, outside my lane.
- **"Mock nodes".** The epic's Scope says "mock STT, LLM and TTS nodes" and its Out of scope excludes nodes, ports and the runtime. The stories deliver mock adapters. Only `story:audio-and-stt-contract` Out of scope says "Wrapping the mock as a graph node with ports (`epic:graph-minimum`)"; the LLM and TTS stories do not. The mismatch comes from the epic's own wording, so I did not count it.
- **`ess-conformance` at tag 0.60.0.** I did not inspect it, so the executability of the `Runner` plus results-document route rests on the testkit story's own citation of `runner.rs`. Whether those steps pass is outside my lane.

```findings
[]
```
