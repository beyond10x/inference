---
format: aep.planning-md/3
id: review-result:contracts-scope-round-1
kind: review-result
status: active
title: aep:plan-critic-scope on epic:contracts-and-testkit, round 1
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

epic:contracts-and-testkit — "`inference-audio`: frames, formats, sample clocks, timebases (§6.1, §11.5)" is promised, but no story puts a timebase in `inference-audio`. `story:inbound-text-spans-and-envelope` steps 1 and 3 declare the timebase enum in `ess/domains/stream.yaml` and `inference-core`. `story:audio-and-stt-contract` step 2 lists only frame, format and sample-clock position, and only refers to "the timebases the envelope … names". The relocation or narrowing is recorded nowhere, and `story:audio-and-stt-contract` is the natural story to claim it or say why it dropped it. — .engineering/planning/epic/contracts-and-testkit.md:37

epic:contracts-and-testkit — "`inference-core`: … typed errors, control events" is promised, and no story declares control events. The only candidate is the envelope's `kind` "separating control from content" (`story:inbound-text-spans-and-envelope` step 3). That is a discriminator, not the Start/stop/commit/revise/cancel/error events of §6.1 (`docs/design/inference.md:293`). The epic lists the envelope and control events as separate items. `story:workspace-skeleton` or `story:inbound-text-spans-and-envelope` should claim them or record the omission. — .engineering/planning/epic/contracts-and-testkit.md:35

story:testkit-conformance-target — The Exit says the suite is "executed by the testkit's own Rust runner and reported through `ess verify conform report --suite --results`". No story runs or mentions `report`. This story substitutes the `ess-conformance` in-process runner and its own report/2 document, and states only the `run`-has-no-Rust-target limitation, not the substitution of `report`. The body should either invoke `report --suite --results` or record why the in-process report replaces it. — .engineering/planning/epic/contracts-and-testkit.md:46-47

What I read:
- 8 artifacts: the epic and its 7 stories, each with `aep plan artifact show <id>`.
- `aep plan artifact graph`, `docs/design/inference.md` lines 280-335, 636-660 and 1098-1140, `ess verify conform report --help`, `.engineering/drafts/suite.json`, and the epic file with line numbers.
- Promises extracted from the epic: 14 (the Outcome's two clauses, five Scope bullets split into 8 sub-promises, three Exit bullets, plus the no-concrete-implementation constraint). I traced 11 to an item. The 3 not traced are the findings above.
- Mechanical check: the 60 `inference.stream` ids listed by the outbound and inbound stories equal the suite's 60 exactly, with no overlap. The 3 `inference.model` ids are claimed by the testkit story. §23.1 tests 1, 2, 3 and 6 map one each to the STT, LLM, TTS and inbound stories. A-01 maps to the STT story.

What I could not establish:
- Whether `ess-conformance` produces a report acceptable in place of `ess verify conform report`. I did not inspect that crate, so the third finding rests on the stories' silence.
- Whether `ess/SKIPPED.md` is outside the epic's reach. It does not exist yet. The testkit story assigns skip reasons to sibling epics (`epic:graph-minimum`, `epic:tools-and-turns`, `epic:session-facade`) and requires later stories to edit it. I did not count that as a finding because the epic does not exclude it, but it looks like a cross-epic obligation the epic never asked for.
- Out of my lane: the new `inference.audio` domain and the envelope declaration may change the suite counts after re-synthesis. That is a design question, and I did not rule on it.

```findings
- file: .engineering/planning/epic/contracts-and-testkit.md
  line: 37
  category: scope
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "epic:contracts-and-testkit — 'inference-audio: frames, formats, sample clocks, timebases (§6.1, §11.5)' is promised and no story puts a timebase in inference-audio; the timebase enum lands in inference-core via story:inbound-text-spans-and-envelope and story:audio-and-stt-contract only refers to it, so the relocation or narrowing is unrecorded (natural owner: story:audio-and-stt-contract)"
- file: .engineering/planning/epic/contracts-and-testkit.md
  line: 35
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "epic:contracts-and-testkit — 'inference-core: … typed errors, control events' is promised and no story declares control events; the envelope's kind discriminator in story:inbound-text-spans-and-envelope step 3 is not the Start/stop/commit/revise/cancel/error set of §6.1 (natural owner: story:workspace-skeleton or story:inbound-text-spans-and-envelope)"
- file: .engineering/planning/epic/contracts-and-testkit.md
  line: 46
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "story:testkit-conformance-target — the Exit promises the suite is 'reported through `ess verify conform report --suite --results`' by the testkit's own Rust runner, and the story neither runs that command nor records why the ess-conformance in-process report replaces it"
```
