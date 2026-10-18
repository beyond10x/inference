---
format: aep.planning-md/3
id: review-result:contracts-design-round-2
kind: review-result
status: active
title: aep:plan-critic-design on epic:contracts-and-testkit, round 2
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

**What I read:** 8 artifacts (the epic and the 8 stories under it, counting `story:audio-vocabulary`; that is 9 bodies in total). I read each in full with `aep plan artifact show`, plus the round-1 record `review-result:contracts-design-round-1`. I ran `aep plan artifact relations`, `aep plan artifact graph` and `aep plan artifact validate` (valid, 55 artifacts). I also read the `ess-conformance` 0.60.0 source locally.

Round-1 findings, both fixed:
- **Round-1 finding 1** (`story:audio-and-stt-contract` held the audio vocabulary and the STT contract, so `story:tts-contract` queued behind six stories) is fixed.
  - `story:audio-vocabulary` exists and depends only on `story:workspace-skeleton`.
  - It declares frame time itself ("this story declares frame time itself and needs no edge to `story:inbound-text-spans-and-envelope`").
  - `story:tts-contract` now has `depends_on` edges to `story:audio-vocabulary` and `story:outbound-text-generations-and-segments`, and its body says it depends on "nothing of the inbound-text or STT stories".
  - `story:audio-and-stt-contract` now depends on both `story:inbound-text-spans-and-envelope` and `story:audio-vocabulary`, and its out-of-scope line sends the audio frames to `story:audio-vocabulary`.
  - The longest chain is now skeleton, testkit, outbound, inbound, STT. LLM and TTS branch off outbound, and the audio vocabulary branches off the skeleton.
- **Round-1 finding 2** (three bodies shared ownership of the testkit feature scheme) is fixed.
  - `story:testkit-conformance-target`, step 1, now owns it: it declares `stt = []`, `llm = []` and `tts = []` and the empty feature-gated modules.
  - Its step 4 owns the `--all-features` flags in `Taskfile.yml`.
  - Each contract story now only replaces its own line, at `story:audio-and-stt-contract` step 2, `story:llm-contract` step 2 and `story:tts-contract` step 2. The epic scope line agrees.

**How far I walked:**
- **Edges:** I walked the full graph output, all 55 artifacts. There is no cycle and the epic carries no `depends_on`.
- **Inside the set:** 9 `depends_on` edges.
- **From outside into the set:** 14 edges, from `story:pipeline-declaration`, `story:bounded-edges`, `story:endpoint-controller`, `story:tool-call-lifecycle`, `story:text-pipeline-end-to-end` and four epics. All are one-way into this set, so there is no return path.
- **Acceptance reads:** I traced 14 reads to a producer in the set, and an edge records every one (4 of them only transitively):
  - `ess/SKIPPED.md`
  - the `inference-core` constructors
  - the testkit `stream` module and `tests/target/`
  - `Generation` and `OutputSegment`
  - the envelope
  - `inference-audio`
  - the three testkit features
  - the `Taskfile.yml` check task
- **Outside the set:** the sibling stories' claims about this set (`story:pipeline-declaration` ordered behind the inbound and LLM stories) match the recorded edges.
- **Scenario counts:** 46 + 14 = 60 stream scenarios, matching the epic.

**What I could not establish:**
- **Event-log owner (design, weak).** `story:inbound-text-spans-and-envelope` step 4 wraps "every recorded event" of "the `stream` store module the outbound story opened". `story:outbound-text-generations-and-segments` step 3 never says it records generation and segment events in order. The testkit story's model store does record events in order, so I did not treat this as a finding.
- **`inference.audio` domain (acceptance and parallel-safety, not mine).**
  - `story:audio-vocabulary` adds a seventh domain to `ess/system.yaml` with no edge to `story:testkit-conformance-target`. That story's step 1 enumerates six domains and compares `unsupported` against `ess/SKIPPED.md`.
  - The `ess-conformance` 0.60.0 source doc names five scenario families, and none comes from a `types:` entry, so the "153 stay" claim is plausible. I did not run the re-synthesis.
  - Both `story:audio-vocabulary` and `story:inbound-text-spans-and-envelope` write `.engineering/drafts/suite.json` and the specification artifact body.
- **`Cargo.toml` and the Taskfile (parallel-safety, not mine).**
  - Three contract stories and `story:pipeline-declaration` edit the testkit `Cargo.toml` tables.
  - `story:pipeline-declaration` has no edge to the STT or TTS stories.
  - `story:testkit-conformance-target` and `story:audio-vocabulary` both reach the `check` task.
- **Stale text in a sibling (not mine).** `story:pipeline-declaration` still says `story:llm-contract` "adds the `llm` feature". The testkit story now declares it.

```findings
[]
```
