---
format: aep.planning-md/3
id: review-result:contracts-parallel-round-1
kind: review-result
status: active
title: aep:plan-critic-parallel-safety on epic:contracts-and-testkit, round 1
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

story:testkit-conformance-target — story:pipeline-declaration (epic:graph-minimum) reads the conformance runner this story creates and passes its scenarios through it, yet its only `depends_on` edge is to `story:workspace-skeleton`, so the two sit in one wave and both land on `crates/inference-testkit/tests/conform.rs` and `ess/SKIPPED.md`; the missing edge is pipeline-declaration → testkit-conformance-target (it lives on pipeline-declaration, whose line 31 still says the edge should move once a skeleton story exists) — .engineering/planning/story/pipeline-declaration.md:31,39, .engineering/planning/story/testkit-conformance-target.md:27-28
story:testkit-conformance-target — story:tool-call-lifecycle (epic:tools-and-turns) has no `depends_on` edge at all, yet its body extends this story's target to answer `inference.tools` and so lands on `conform.rs` and `ess/SKIPPED.md`; the missing edge is tool-call-lifecycle → testkit-conformance-target — .engineering/planning/story/tool-call-lifecycle.md:66, .engineering/planning/story/testkit-conformance-target.md:27-28
story:outbound-text-generations-and-segments — this and story:pipeline-declaration have no ordering between them (both depend only upward on the skeleton or testkit story), and both add dispatch arms to `crates/inference-testkit/tests/conform.rs`, edit `ess/SKIPPED.md` and add a module to `crates/inference-core` (cited, both bodies), and neither says so; remedy is either an edge recording the shared files or splitting the target per domain module — .engineering/planning/story/outbound-text-generations-and-segments.md:28,30, .engineering/planning/story/pipeline-declaration.md:29,39
story:inbound-text-spans-and-envelope — this and story:pipeline-declaration both land on `crates/inference-testkit/tests/conform.rs`, `ess/SKIPPED.md` and `crates/inference-core` (cited, both bodies; the envelope against the `inference.policy` types) with no edge between them and neither body saying so; remedy is either an ordering edge or splitting the target per domain module — .engineering/planning/story/inbound-text-spans-and-envelope.md:31,32, .engineering/planning/story/pipeline-declaration.md:29,39
story:inbound-text-spans-and-envelope — story:endpoint-controller (epic:tools-and-turns) has no `depends_on` edge, yet its acceptance passes `inference.stream.OpenTurn/outcome/opened` and `inference.stream.CompleteTurn/outcome/completed`, which this story makes answerable, so the reader is scheduled beside its producer; the missing edge is endpoint-controller → inbound-text-spans-and-envelope — .engineering/planning/story/endpoint-controller.md:50, .engineering/planning/story/inbound-text-spans-and-envelope.md:37
story:audio-and-stt-contract — story:endpoint-controller takes audio frames in from `inference-audio`, which this story creates, with no `depends_on` edge, so the reader is scheduled beside its producer; the missing edge is endpoint-controller → audio-and-stt-contract — .engineering/planning/story/endpoint-controller.md:26, .engineering/planning/story/audio-and-stt-contract.md:31
story:llm-contract — this and story:audio-and-stt-contract (and story:tts-contract) are not ordered and each lands on `crates/inference-testkit/Cargo.toml` `[features]` and `crates/inference-testkit/src/lib.rs` (inferred for lib.rs, cited for the features): audio line 20 names all three features `stt`, `llm`, `tts` while llm adds `llm` and tts adds `tts`, and llm's only independence claim covers the inbound-text story; the body says nothing about audio or tts, and it is unclear who creates the `llm` feature; remedy is either an ordering edge or one story declaring all three features — .engineering/planning/story/llm-contract.md:18,28, .engineering/planning/story/audio-and-stt-contract.md:20,33, .engineering/planning/story/tts-contract.md:28
story:workspace-skeleton — step 1 says "members under `crates/`" without saying whether that is a glob, so outbound (`inference-text`), audio (`inference-audio`, `inference-stt` and `examples/stt-only`, which is outside `crates/`), llm, tts and pipeline-declaration may each have to edit the root `Cargo.toml` members list (inferred); the body should state the member globs it sets (`crates/*`, `examples/*`) or name who adds each member — .engineering/planning/story/workspace-skeleton.md:27

**What you read.** 7 set stories, the epic, `epic:graph-minimum`, `epic:tools-and-turns`, and the sibling stories `pipeline-declaration`, `tool-call-lifecycle` and `endpoint-controller`. Commands: `aep plan artifact show <id>` for each, `aep plan artifact list --format json`, `aep plan artifact waves --kind story --status draft` (0 waves, 21 unassessed, no collisions computable), `aep plan artifact waves --help`, and `grep -n` over the story files for line citations.

- **Placement.** 7 of 7 set stories are placed by cited crate or directory paths. 0 are inferred and 0 are unplaceable. File-level placement is inferred for `src/lib.rs` and for the root `Cargo.toml` members list, which no body names.
- **Ordering inside the set.** The `depends_on` chain is skeleton → testkit → outbound → inbound → audio → tts, plus outbound → llm. Every pair on the chain is ordered, so sharing the testkit, core, `SKIPPED.md` or Taskfile surfaces there is not a finding. The only concurrent pairs are llm with inbound, audio and tts.
- **Reader beside producer.** 7 acceptance reads traced to a producer inside the set, 7 of them recorded by a `depends_on` edge (tts reads outbound's segments through the chain). 3 more reads from sibling stories have no edge: pipeline-declaration and tool-call-lifecycle on the testkit target, and endpoint-controller on inbound and audio. That makes 4 findings, since endpoint-controller counts twice.

**What I could not establish.**
- Whether the epic-level edges (`epic:graph-minimum` and `epic:tools-and-turns` → `epic:contracts-and-testkit`, directly or through `local-voice-cascade`) are meant to keep the sibling stories from starting early. `aep plan artifact waves` reads story `depends_on` and `scope` only, so the store would still put pipeline-declaration, tool-call-lifecycle and endpoint-controller in the first wave. The findings treat story edges as the ordering that counts. Pipeline-declaration's own line 31 confirms the story edge is the intended gate.
- Whether `story:tool-call-lifecycle` will actually move tool types out of `inference-llm`. Its line 63 conditions this on "any tool types P0 left", and llm-contract's out-of-scope clause leaves none, so I did not file it.
- Out of my lane: whether the testkit feature set (`stt`, `llm`, `tts`) is the right split (design critic). Whether the `--all-features` Taskfile change in audio step 6 is checkable (acceptance critic).

```findings
- file: .engineering/planning/story/pipeline-declaration.md
  line: 31
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "story:testkit-conformance-target (named here): story:pipeline-declaration reads the conformance runner that story creates and passes its scenarios through it, yet its only depends_on edge is to story:workspace-skeleton, so both land in one wave on crates/inference-testkit/tests/conform.rs and ess/SKIPPED.md; the missing edge is pipeline-declaration to testkit-conformance-target (cited, both bodies)"
- file: .engineering/planning/story/tool-call-lifecycle.md
  line: 66
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "story:testkit-conformance-target (named here): story:tool-call-lifecycle has no depends_on edge yet extends that story's target to answer inference.tools, landing on crates/inference-testkit/tests/conform.rs and ess/SKIPPED.md; the missing edge is tool-call-lifecycle to testkit-conformance-target (cited, both bodies)"
- file: .engineering/planning/story/outbound-text-generations-and-segments.md
  line: 30
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "this story and story:pipeline-declaration are unordered and both add dispatch arms to crates/inference-testkit/tests/conform.rs, edit ess/SKIPPED.md and add a module to crates/inference-core (cited, both bodies); neither says so; remedy is an ordering edge recording the shared files or splitting the target per domain module"
- file: .engineering/planning/story/inbound-text-spans-and-envelope.md
  line: 32
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "this story and story:pipeline-declaration are unordered and both land on crates/inference-testkit/tests/conform.rs, ess/SKIPPED.md and crates/inference-core (cited, both bodies); neither says so; remedy is an ordering edge recording the shared files or splitting the target per domain module"
- file: .engineering/planning/story/inbound-text-spans-and-envelope.md
  line: 37
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "story:endpoint-controller has no depends_on edge yet its acceptance passes inference.stream.OpenTurn/outcome/opened and inference.stream.CompleteTurn/outcome/completed, which this story makes answerable (reader beside producer); the missing edge is endpoint-controller to inbound-text-spans-and-envelope"
- file: .engineering/planning/story/audio-and-stt-contract.md
  line: 31
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "story:endpoint-controller takes audio frames in from inference-audio, which this story creates, with no depends_on edge (reader beside producer); the missing edge is endpoint-controller to audio-and-stt-contract"
- file: .engineering/planning/story/llm-contract.md
  line: 28
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "this story and story:audio-and-stt-contract (and story:tts-contract) are unordered and each lands on crates/inference-testkit/Cargo.toml features (cited) and src/lib.rs (inferred): audio line 20 names all three features stt, llm, tts while this story adds llm and tts adds tts, and the independence claim at line 18 covers only the inbound-text story; remedy is an ordering edge or one story declaring all three features"
- file: .engineering/planning/story/workspace-skeleton.md
  line: 27
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "step 1 says members under crates/ without saying whether that is a glob, so outbound, audio (including examples/stt-only outside crates/), llm, tts and pipeline-declaration may each edit the root Cargo.toml members list (inferred); the body should state the member globs it sets or name who adds each member"
```
