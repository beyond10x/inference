---
format: aep.planning-md/3
id: review-result:contracts-design-round-1
kind: review-result
status: active
title: aep:plan-critic-design on epic:contracts-and-testkit, round 1
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

story:audio-and-stt-contract — two things in one item: `inference-audio` (frames, formats, clocks), which `story:tts-contract` needs, and the STT contract, A-01 example and mock recogniser, which nothing else in the set needs. Because `story:tts-contract` has to depend on this story for the audio crate, it sits behind `story:inbound-text-spans-and-envelope`, `story:outbound-text-generations-and-segments`, `story:testkit-conformance-target` and `story:workspace-skeleton`, so six of the seven stories form one chain and only `story:llm-contract` runs beside it. Splitting the audio vocabulary into its own story would cut the TTS path to the stories it uses; the `depends_on` edge from tts would then point at the audio story, not the STT one. The audio story's frame position is "one of the timebases the envelope of the previous story names", so the split story must either keep a `depends_on` edge to `story:inbound-text-spans-and-envelope` for the timebase enum or declare frame time itself. — `aep plan artifact graph` (`story:tts-contract -> story:audio-and-stt-contract -> story:inbound-text-spans-and-envelope -> story:outbound-text-generations-and-segments -> story:testkit-conformance-target -> story:workspace-skeleton`); `.engineering/planning/story/audio-and-stt-contract.md:18` and `.engineering/planning/story/tts-contract.md:10`

story:audio-and-stt-contract — it owns the testkit feature scheme for all three mocks: it says the mocks sit behind features `stt`, `llm` and `tts`, each enabling an optional dependency on its contract crate, and `AGENTS.md` "names the three testkit features". It cannot do that, because `inference-llm` has no edge to this story and `inference-tts` arrives after it. `story:llm-contract` and `story:tts-contract` also each introduce their own feature in step 2. Half the feature abstraction therefore sits in three bodies with no stated owner. The body should say that this story adds `stt` and the `--all-features` flag only, and that the llm and tts stories each add their own feature and name it in `AGENTS.md`. — `.engineering/planning/story/audio-and-stt-contract.md:20`, `.engineering/planning/story/audio-and-stt-contract.md:35`, `.engineering/planning/story/llm-contract.md:28`, `.engineering/planning/story/tts-contract.md:28`

**What I read:** all 8 artifacts (the epic and 7 stories) in full with `aep plan artifact show`. I ran `aep plan artifact relations`, `graph` and `validate` (valid) and `list --format json` (output truncated). I walked all 42 artifacts' edges, including those outside the set: no cycle, and the epic carries no `depends_on`. Inside the set there are 6 `depends_on` edges. I traced 9 acceptance reads to a producer in the set, and an edge, direct or transitive, records every one:
- `ess/SKIPPED.md`, created by the testkit story and read by the outbound and inbound stories.
- The `inference-core` constructors, read by the testkit story.
- `OutputSegment`, read by the llm and tts stories.
- The envelope, read by the audio story.
- `inference-audio`, read by the tts story.

I checked the 46 outbound and 14 inbound scenario counts against the story lists: 19 + 27 = 46 and 14 = 14, matching the epic's 60 stream scenarios. No edge or body reference runs from this set to `epic:graph-minimum` or `epic:tools-and-turns`; they appear only as named owners of `ess/SKIPPED.md` lines. The only forward reference is the `port` type in the inbound story, kept opaque until `epic:graph-minimum` settles it.

**What I could not establish:**
- Whether declaring the envelope (inbound step 1) and the audio domain (audio step 1) in `ess/` adds synthesized scenarios. If it does, the testkit equality assertion "`unsupported` equals `ess/SKIPPED.md`" goes red unless those ids are added. That belongs to `plan-critic-acceptance` and was not run.
- Whether the shared files (`ess/SKIPPED.md`, the testkit stores, the Taskfile) collide when inbound runs beside llm. That belongs to `plan-critic-parallel-safety`.
- Epic Exit coverage is out of my lane (scope).

```findings
- file: .engineering/planning/story/audio-and-stt-contract.md
  line: 18
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the story holds two things, the inference-audio vocabulary that story:tts-contract needs and the STT contract with A-01 example that nothing else needs, so tts depends on it and queues behind the inbound, outbound, testkit and skeleton stories; splitting the audio vocabulary into its own story shortens that chain (it keeps a depends_on edge to story:inbound-text-spans-and-envelope for the timebase enum, or declares frame time itself)"
- file: .engineering/planning/story/audio-and-stt-contract.md
  line: 20
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the body claims the feature scheme for stt, llm and tts and AGENTS.md naming of all three, but story:llm-contract has no edge to it and story:llm-contract and story:tts-contract each add their own feature in step 2, so the feature abstraction is split across three bodies with no owner; the body should add only stt and --all-features and leave llm and tts to their stories"
```
