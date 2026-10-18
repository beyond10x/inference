---
format: aep.planning-md/3
id: review-result:contracts-parallel-round-2
kind: review-result
status: active
title: aep:plan-critic-parallel-safety on epic:contracts-and-testkit, round 2
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

story:audio-vocabulary — it and story:inbound-text-spans-and-envelope are unordered (this one depends only on story:workspace-skeleton) and both rewrite the same facts of `executable-system-specification:inference` ("six domains" on line 19, the "What it declares" counts on line 33, the synthesize row on line 41 and the summary on line 7), each recording counts from its own re-synthesis, so whichever lands second overwrites the first or records a total that does not include it; neither body names the other or the section it writes (cited, both bodies); remedy is either an ordering edge between the two that records the specification artifact as its reason, or each story recording its counts in a section of its own — .engineering/planning/story/audio-vocabulary.md:18,28, .engineering/planning/story/inbound-text-spans-and-envelope.md:18,31, .engineering/planning/executable-system-specification/inference.md:7,19,33,41

story:tts-contract — it and story:pipeline-declaration are unordered (tts depends on audio-vocabulary and outbound, pipeline-declaration on skeleton, testkit, inbound and llm) and both edit `crates/inference-testkit/Cargo.toml` (pipeline-declaration adds its `inference-pipeline` dependency line there, cited, line 36); the body claims the three contract stories share "two lines ... and nothing else" and does not name pipeline-declaration, which in turn lists only outbound, inbound and llm and not tts; remedy is either an ordering edge recording the shared file as its reason, or splitting the surface so the two no longer share it — .engineering/planning/story/tts-contract.md:21, .engineering/planning/story/pipeline-declaration.md:36

**Round 1 status.** I do not repeat the eight round-1 findings; each is fixed or now admitted:
- **pipeline-declaration → testkit, and its place behind outbound and inbound:** fixed. Frontmatter `depends_on` lines `pipeline-declaration.md:11-14` now name skeleton, testkit, inbound and llm. Outbound and inbound sit behind it through the inbound edge.
- **tool-call-lifecycle → testkit:** fixed (`tool-call-lifecycle.md:11`).
- **endpoint-controller → inbound and → audio-stt:** fixed (`endpoint-controller.md:11-12`).
- **Per-domain targets and SKIPPED sections:** `conform.rs` is no longer edited by later stories, which fill their own `tests/target/<domain>.rs` and `src/<domain>.rs` (`testkit-conformance-target.md:28`; `pipeline-declaration.md:44`).
- **Skeleton root `Cargo.toml` members:** fixed. Step 1 now sets the globs `crates/*` and `examples/*` (`workspace-skeleton.md:27`) and names audio-and-stt-contract as the one adder of `examples/*`.
- **llm, audio and tts, three parts:**
  - Who creates the `llm` feature: fixed. The testkit story declares `stt = []`, `llm = []` and `tts = []` in step 1 (`testkit-conformance-target.md:28`).
  - Who edits `src/lib.rs`: fixed. The modules are pre-declared empty, so no later story edits `lib.rs`.
  - The three sharing two lines of testkit `Cargo.toml`: now admitted in all three bodies (`llm-contract.md:18`, `audio-and-stt-contract.md:30`, `tts-contract.md:21`). That is a named collision, not a finding. The part still open is the tts and pipeline-declaration pair above.

**What I read.** 8 set stories, the epic, the round-1 review-result, `executable-system-specification:inference`, and the sibling stories pipeline-declaration, tool-call-lifecycle, endpoint-controller and bounded-edges (plus a grep of pipeline-validation). Commands:
- `aep plan artifact show` on each of these.
- `aep plan artifact list --format json`, for the `depends_on` edges.
- `aep plan artifact waves --kind story --status draft`: no wave, every story unassessed.
- `grep -n` over `.engineering/planning/story/*.md`, for line citations.
- `git check-ignore`, which shows `.engineering/drafts/` is gitignored.

Placement: 8 of 8 set stories **cited** by crate, directory and `ess/` paths, 0 inferred, 0 unplaceable. The file-level `Cargo.toml` `[dependencies]` surface below is inferred. Acceptance reads traced to a producer inside the set: 11, all 11 recorded by a `depends_on` edge, directly or through the chain skeleton → testkit → outbound → {inbound, llm, tts}. A further 9 reads from the four siblings are all recorded by an edge. Concurrent pairs inside the set: audio-vocabulary with testkit, outbound, inbound and llm; tts with inbound, llm and audio-stt. Their only shared files are the two findings above and the admitted testkit `Cargo.toml` lines.

**What I could not establish.**
- **Testkit `[dependencies]` (inferred).** Outbound would add `inference-text` and tool-call-lifecycle would add `inference-tools` to the testkit `[dependencies]` table. No body names this, and the two are unordered (tool-call-lifecycle depends only on testkit). I left it out because I could not cite either body adding the line.
- **Adjacent lines.** Whether the three admitted `Cargo.toml` edits (`stt = []`, `llm = []`, `tts = []` plus the optional dependency lines) would conflict on a textual merge as adjacent lines. The file does not exist yet, so "may merge in any order" (`llm-contract.md:18`) cannot be tested.
- **Which store section each writer uses.** Whether `aep plan artifact body --section` lets audio-vocabulary and inbound write different sections of the specification artifact. The bodies name none, which is why the first finding stands.
- **tool-call-lifecycle step 6.** "record the new scenario count in this body" (`tool-call-lifecycle.md`, step 6) is ambiguous between the story and the specification artifact. If it means the specification artifact, it joins the first finding.
- **Wave tooling.** `waves` still computes nothing because no `scope` is recorded. All concurrency above is read from bodies and `depends_on` edges.
- **Epic edges.** The epic-level edges from `epic:graph-minimum` and `epic:tools-and-turns` (via `local-voice-cascade`) are not read by the story-level `waves`, as in round 1.
- **Out of my lane.**
  - `pipeline-declaration.md:36` says the stream stories each add a `mod` line to `src/lib.rs`. The testkit story pre-declares every module so that no later story touches `lib.rs` (`testkit-conformance-target.md:28`). That is a consistency matter for the design critic.
  - audio-vocabulary adds an `inference.audio` domain while the testkit lists six domain modules. Also design.

```findings
- file: .engineering/planning/story/audio-vocabulary.md
  line: 28
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "story:inbound-text-spans-and-envelope (named here): this story depends only on story:workspace-skeleton, so it is unordered with story:inbound-text-spans-and-envelope, and both rewrite the same facts of executable-system-specification:inference (six domains on line 19, counts on line 33, synthesize row on line 41, summary on line 7) from their own re-synthesis, so the later writer overwrites the earlier or records a total missing it; neither body names the other or the section it writes (cited, both bodies); remedy is either an ordering edge recording the specification artifact as its reason or each story recording its counts in its own section"
- file: .engineering/planning/story/tts-contract.md
  line: 21
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "story:pipeline-declaration (named here): this story and story:pipeline-declaration are unordered and both edit crates/inference-testkit/Cargo.toml (cited, pipeline-declaration.md:36 adds its inference-pipeline dependency there); this body claims the three contract stories share two lines and nothing else and does not name pipeline-declaration, which lists only outbound, inbound and llm; remedy is either an ordering edge recording the shared file as its reason or splitting the surface so the two no longer share it"
```
