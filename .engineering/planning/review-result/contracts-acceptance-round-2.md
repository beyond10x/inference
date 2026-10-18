---
format: aep.planning-md/3
id: review-result:contracts-acceptance-round-2
kind: review-result
status: active
title: aep:plan-critic-acceptance on epic:contracts-and-testkit, round 2
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
needs-revision

story:audio-vocabulary — the acceptance tests only "format equality and frame duration arithmetic", so the sample clock position, which Step 2 and the epic's narrowed `inference-audio` item name as the audio-frame-time deliverable, has no check of its own; it should name a test of the position-to-time conversion at the format's sample rate — .engineering/planning/story/audio-vocabulary.md:33

Round-1 findings, one by one:
- `story:workspace-skeleton` (round-1 line 34) — fixed. The acceptance at `.engineering/planning/story/workspace-skeleton.md:34` now says the three unit tests of `## Checks` "together observe the `inference.model` vocabulary". Those checks cover the `DeclareModel` entity-plus-event return (line 38), `DeclareAdapter` (line 39) and enum plus identifier round-trips (Check 3, line 40).
- `story:outbound-text-generations-and-segments` (round-1 line 34) — fixed. The acceptance at `.engineering/planning/story/outbound-text-generations-and-segments.md:34` now points at "the 46 ids listed under `## Scenarios`", and the unreproducible 19 and 27 are gone. Those 46 ids all exist in the suite, and with the inbound story's 14 they partition the 60 stream ids with no overlap.
- `story:inbound-text-spans-and-envelope` (round-1 line 37) — fixed. The acceptance at `.engineering/planning/story/inbound-text-spans-and-envelope.md:39` now ends "on a tree where the declaration check of `## Checks` holds". Check 1 (line 43) names the `types:` struct and enum entries in `ess/domains/stream.yaml`, `ess specify validate` exit 0 and `synthesize` reporting 0 refusals.
- `story:audio-and-stt-contract` (round-1 line 39) — fixed. The audio-nouns declaration moved to `story:audio-vocabulary`, whose acceptance at `.engineering/planning/story/audio-vocabulary.md:33` names `ess/domains/audio.yaml` existing, listed in `ess/system.yaml` and `ess/ess-inputs.yaml`, and validating. The STT acceptance (line 35) now covers only what the story builds.

The `story:audio-vocabulary` finding is new: it is not a round-1 finding, and the story did not exist then. The epic, the testkit, LLM and TTS stories carry no finding.

**What I read:** 9 of 9 ids, whole bodies, via `aep plan artifact show`: the epic, the 8 stories and `review-result:contracts-acceptance-round-1`. I also ran `aep plan artifact kinds` and `aep plan artifact lifecycle story`, and read the procedure and the rubric. I checked the 46 and 14 scenario ids in the outbound and inbound stories against `.engineering/drafts/suite.json`: all present, and the suite has 153 scenarios, 3 `inference.model` and 60 `inference.stream`. Neither `ess/domains/audio.yaml` nor the envelope exists in `ess/` yet, which fits the stories being drafts.

**What I could not establish:**
- Whether `task check` can run the `ess-conformance` dev-dependency and `ess verify conform synthesize` in the test environment. No code exists, so I ran none of the cargo or ess checks.
- Whether the declarations in `story:audio-vocabulary` and `story:inbound-text-spans-and-envelope` add synthesized scenarios. Both stories say the suite stays at 153 and record the counts either way, so I treat that as a red test rather than a gap.
- Out of my lane (design and parallel-safety): the new `depends_on` edges (STT depends on audio-vocabulary and inbound; TTS depends on audio-vocabulary and outbound), and the shared `crates/inference-testkit/Cargo.toml` and `ess/SKIPPED.md`.

```findings
- file: .engineering/planning/story/audio-vocabulary.md
  line: 33
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance tests only format equality and frame duration arithmetic, so the sample clock position, the audio-frame-time deliverable of Step 2 and of the epic's narrowed inference-audio item, has no check of its own; it should name a test of the position-to-time conversion at the format's sample rate"
```
