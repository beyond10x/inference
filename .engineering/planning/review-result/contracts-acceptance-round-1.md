---
format: aep.planning-md/3
id: review-result:contracts-acceptance-round-1
kind: review-result
status: active
title: aep:plan-critic-acceptance on epic:contracts-and-testkit, round 1
relations:
- reviews: epic:contracts-and-testkit
- reviews: story:workspace-skeleton
- reviews: story:testkit-conformance-target
- reviews: story:outbound-text-generations-and-segments
- reviews: story:inbound-text-spans-and-envelope
- reviews: story:audio-and-stt-contract
- reviews: story:llm-contract
- reviews: story:tts-contract
revision: 1
---
needs-revision

story:workspace-skeleton — the acceptance's clause "declares the `inference.model` vocabulary" has no check of its own, because the one test it names (`DeclareAdapter` refused at 0) leaves `DeclareModel`, the events, the nine capabilities and the shared identifiers unobserved; it should name the step-2 enum-serialization test and the `DeclareModel` entity-plus-event return — .engineering/planning/story/workspace-skeleton.md:34
story:outbound-text-generations-and-segments — the acceptance counts "the 19 `inference.stream.Generation` and the 27 `inference.stream.OutputSegment` scenarios", but the ids carrying those prefixes in the suite number 12 and 18, so a reader who counts by prefix cannot reproduce the figures; it should point at the 46 ids under `## Scenarios` — .engineering/planning/story/outbound-text-generations-and-segments.md:34
story:inbound-text-spans-and-envelope — the acceptance observes none of step 1's deliverable, the envelope and timebase declared in `ess/`, so it reads the same whether or not they were declared (the `ess ... validate` and `ess ... synthesize` calls in `task check` don't tell the two apart); it should name a before/after check such as `ess specify validate --path ess` exit 0 with the envelope `types:` entry present in `ess/domains/stream.yaml` — .engineering/planning/story/inbound-text-spans-and-envelope.md:37
story:audio-and-stt-contract — the acceptance observes none of step 1's deliverable, the audio nouns declared in `ess/` (`ess/domains/audio.yaml` listed in `ess/system.yaml`), so it can be met with that file absent; it should name that file and a validate exit 0 — .engineering/planning/story/audio-and-stt-contract.md:39

**What I read:** 7 of 7 ids (epic:contracts-and-testkit and the 6 stories), whole bodies, via `aep plan artifact show <id>`. I also ran `aep plan artifact kinds`, `aep plan artifact lifecycle story` and `aep plan artifact validate` (42 artifacts, valid). I checked every scenario id named across the set against `.engineering/drafts/suite.json` and none is missing. The counts check out: 153 in total, 3 model and 60 stream. The 46 outbound ids and 14 inbound ids partition the 60 stream ids exactly. The workspace and testkit acceptances, the llm and tts acceptances and the epic's Exit are observable and carry their transition, so they have no finding.

**What I could not establish:**
- `AGENTS.md` Rules says "A story's acceptance is a set of named conformance scenarios". The audio, llm and tts acceptances name Rust tests only, because `ess/` declares nothing for those contracts yet. I did not treat that as a finding.
- The epic's Exit says the verdict is reported through `ess verify conform report --suite --results`. The testkit story asserts on the report document from inside the Rust test and never runs that command. Whether this is a gap in the epic is outside my lane (scope).
- Whether declaring the envelope or audio types in `ess/` adds synthesized scenarios, which would make the `unsupported` set differ from the ids in `ess/SKIPPED.md`. Neither story says so. It would be a red test, not a missing acceptance, so I did not make it a finding.
- I did not run the cargo or `cargo tree` checks, since no code exists. For the A-01 assertion, `inference-tts` and `inference-runtime` don't exist yet while the audio story is open. I did not check that `cargo tree -p stt-only` is unaffected by the workspace-wide `--all-features` feature unification.
- Out of my lane (design and parallel-safety): the dependency order tts → audio → inbound → outbound, and which story edits the shared `ess/SKIPPED.md` and `Taskfile.yml`.

```findings
- file: .engineering/planning/story/workspace-skeleton.md
  line: 34
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance clause 'declares the inference.model vocabulary' has no check of its own, since the one test it names (DeclareAdapter refused at max_concurrency 0) leaves DeclareModel, the events, the nine capabilities and the shared identifiers unobserved"
- file: .engineering/planning/story/outbound-text-generations-and-segments.md
  line: 34
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance counts 19 inference.stream.Generation and 27 inference.stream.OutputSegment scenarios, but the ids with those prefixes in .engineering/drafts/suite.json number 12 and 18, so the counts cannot be reproduced; it should point at the 46 ids under ## Scenarios"
- file: .engineering/planning/story/inbound-text-spans-and-envelope.md
  line: 37
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance observes none of step 1's deliverable, the envelope and timebase declared in ess/, so it reads the same whether or not they were declared; it names no validate exit or declaration to find in ess/domains/stream.yaml"
- file: .engineering/planning/story/audio-and-stt-contract.md
  line: 39
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance observes none of step 1's deliverable, the audio nouns declared in ess/ (ess/domains/audio.yaml listed in ess/system.yaml), so it can be met with that file absent"
```
