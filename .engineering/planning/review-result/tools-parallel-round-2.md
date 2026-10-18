---
format: aep.planning-md/3
id: review-result:tools-parallel-round-2
kind: review-result
status: active
title: aep:plan-critic-parallel-safety on epic:tools-and-turns, round 2
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

endpoint-controller — it and story:tool-authority-and-executor-nodes both add new files under `crates/inference-pipeline/src/nodes/` and `crates/inference-testkit/src/` with no `depends_on` edge between them, so one wave holds both. Neither names the file that declares those modules, and neither admits sharing it. Inferred: no story creates `src/nodes/mod.rs`, so both would create it, and that file does not exist yet. The testkit `src/lib.rs` is also inferred, since each new top-level file needs a `mod` line, even though `story:testkit-conformance-target` pre-declares its modules so that "no later story adds a line to `lib.rs`". The `inference-pipeline` `Cargo.toml` dependency lines for `inference-audio` and `inference-tools` are inferred as well. Remedies, not chosen here: an ordering edge that records those files as its reason, or one story creating `nodes/mod.rs` and the testkit module lines so the other adds none. — .engineering/planning/story/endpoint-controller.md:58-71, .engineering/planning/story/tool-authority-and-executor-nodes.md:59-64

**Round-1 findings**
- **`tool-call-interruption`, shared executor files with `tool-safe-retries`: fixed.**
  - A `depends_on` edge now runs from `tool-call-interruption` to `tool-safe-retries`.
  - The ordering is stated and the reason named (executor node, contract, mock): `.engineering/planning/story/tool-call-interruption.md:26-30`.
  - The hold-until-release behaviour is now in this story's scope, with a `cancelled` log entry: `tool-call-interruption.md:51`.
  - The authority story names the same order, `tool-safe-retries` then `tool-call-interruption`: `tool-authority-and-executor-nodes.md:64`.
- **`cascade-read-only-tool`, shared LLM node with `tool-call-interruption`: fixed.**
  - A `depends_on` edge now runs from `cascade-read-only-tool` to `tool-call-interruption`, and both bodies say the order and why: `.engineering/planning/story/cascade-read-only-tool.md:27-30`, `tool-call-interruption.md:32`.
  - The LLM node's file is still named in no body. It is ordered, so I do not report it.
- **`endpoint-controller`, unplaced surface: fixed.**
  - The body now names `nodes/vad.rs`, `nodes/turn_policy.rs`, `src/synthetic_audio.rs` and `tests/endpoint_controller.rs`: `.engineering/planning/story/endpoint-controller.md:58-71`.
  - Line 71 states it is disjoint from `story:tool-call-lifecycle`, which I confirmed: lifecycle edits `tools.yaml`, `inference-tools` and the pre-declared testkit `tools` module.
  - The one remaining collision is the finding above, against the authority story.

**What I read:** 8 artifacts (the epic and 7 stories) and `review-result:tools-parallel-round-1`.
- Commands: `aep plan artifact show` on each, plus `aep plan artifact list --format json` for the `depends_on` edges.
- `aep plan artifact waves` was not rerun; its result is as you described.
- I also searched the 14 sibling story bodies for `nodes/`, `mod.rs`, `lib.rs` and `Cargo.toml`, which is where the testkit `lib.rs` claim comes from.
- Surfaces: cited 7, inferred 0, unplaced 0.
- Acceptance reads traced to a producer in the set: 14. All 14 are ordered by a `depends_on` edge: 8 directly and 6 through the chain lifecycle, authority, feedback or retries.

**What I could not establish**
- The file the LLM node lives in. `story:tool-feedback-loop`, `story:tool-call-interruption` and `story:cascade-read-only-tool` all edit it, but they are ordered by edges and no sibling names the file.
- Whether `tool-safe-retries` changes `ess/domains/tools.yaml` to carry the attempt count on `ToolCallFailed`. Its body names no spec edit, and its revision is still 1. If it does, the change is ordered behind `tool-call-lifecycle` by the chain, and `tool-feedback-loop` does not edit that file.
- Whether `tool-feedback-loop` and `tool-safe-retries` (same wave) share a test file. `tool-feedback-loop` names none and `tool-safe-retries` names none, so I assumed none.
- Whether `endpoint-controller` and `tool-authority-and-executor-nodes` both add dev-dependency features to `crates/inference-runtime/Cargo.toml`. That would be another inferred overlap in the same pair.
- Whether `story:tts-contract`, which `cascade-read-only-tool` reads, is an ancestor of it. Only the epic-level edge through `epic:contracts-and-testkit` links them, and that story is outside the set.
- `epic:local-voice-cascade` has no stories yet, so the reuse of its A-04 test stays unchecked.
- Out of my lane: whether `nodes/` belongs in `inference-pipeline` or `inference-runtime` is a design question. Round 1 raised it and I still cannot place it.

```findings
- file: .engineering/planning/story/endpoint-controller.md
  line: 58
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "this story and story:tool-authority-and-executor-nodes both add new files under crates/inference-pipeline/src/nodes/ and crates/inference-testkit/src/ with no depends_on edge between them, and neither body names the shared module-declaration files (nodes/mod.rs, which no story creates and which does not exist yet, the testkit src/lib.rs, the inference-pipeline Cargo.toml); these registration files are inferred, so order the two with the files as the reason or have one story create them so the other adds none"
```
