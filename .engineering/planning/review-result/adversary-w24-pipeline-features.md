---
format: aep.planning-md/3
id: review-result:adversary-w24-pipeline-features
kind: review-result
status: active
title: Adversary pass on the pipeline node features
relations:
- reviews: story:sdk-model-nodes
revision: 1
---
unit: W24-pipeline-features story:sdk-model-nodes, tree inference-0-24-0-pipeline-features, unit head 96d027b; findings cover 96d027b (+ adversary commit f6630f1)
verdict: nothing found red; 2 doc findings (contract-drift, judgement)
cases: executed 23→25, red 0
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: <scratch> (logs, combos.sh, log.base.rs, log.head.rs), .../W24-pipeline-features/msg.SYayRd
needs-coordinator: the CHANGELOG [Unreleased] line is the coordinator's to fix
```

None of my builds or checks failed, so no red case. The two doc findings are below.

1. **Diff since unit head** (`git diff --stat 96d027b HEAD`): `crates/inference-pipeline/tests/adversary_w24_feature_tree.rs | 85 +`. It is a test file only, committed as f6630f1 by `b10x-bot[bot]` (author and committer).

2. **Case added:** `adversary_w24_feature_tree.rs`, 2 tests, green now. Without features, `cargo tree -p inference-pipeline -e normal --no-default-features` names none of stt, llm, tts and duplex. Each feature adds its own contract and none of the other three. It guards against a missed mutation: if inference-stt or inference-duplex became unconditional again, nothing would notice today. stt-only only checks for tts and llm, and stt-only and the testkit depend on stt and duplex anyway. Proof the guard bites: the same `cargo tree` on a `git archive 5f415a8` copy names all four contracts (deleted afterwards).

3. **Suite run, after the case existed:**
   - `test -p inference-pipeline --lib --test adversary_w24_feature_tree`: 22 passed and 2 passed.
   - `test -p stt-only --test tree`: 1 passed.
   - The 23 before is the implementor's reported 22 + 1. All rc=0.

4. **Judgement findings:**
   - **CHANGELOG.md:11 (contract drift, NEEDS-CHANGE, introduced):** the [Unreleased] Breaking entry still says "`inference-pipeline` depends on `inference-stt`, `inference-llm` (feature `tools`) and `inference-tts`". After this unit they are optional behind features. The entry at :29 (`DuplexNode`) names no `duplex` feature either. The fix is one CHANGELOG line naming the features stt, llm, tts and duplex; the implementor did not supply one.
   - **crates/inference-pipeline/src/nodes/mod.rs:12-17 and lib.rs:1 (judgement, CONFIRMED, introduced):**
     - Neither the crate doc nor the `nodes` module doc says that the model nodes are behind features. The testkit, facade, audio-device and llm crate docs all state their features.
     - Under default features, `cargo doc -p inference-pipeline --no-default-features` gives 6 new unresolved links (`SttNode`, `ModelNode`, `AgentNode`, `TtsNode`, `SpeechNode`, `SttPayload`) and `StreamEmitter` in log.rs. There are 12 more in commit_gate and turn_policy that this unit did not cause.
     - Nothing runs rustdoc in CI and nothing in the workspace uses docsrs, so this is a note.

5. **Attacked, not broken** (134 s of cargo time, about 25 min wall, peak target/ 912M):
   - All 16 subsets of {stt, llm, tts, duplex}: `clippy -p inference-pipeline --no-default-features --all-targets -D warnings` exits 0.
   - Testkit clippy `-D warnings` exits 0 for none, run, duplex, stt, llm, stt+tts and llm+tts.
   - Each dependent checked alone through its own manifest, all exit 0:
     - `--tests` for runtime, facade, llama-server, and audio-device (`--no-default-features --features pipeline`).
     - The libraries of local-voice-cascade, realtime-voice and repo-podcast.
     - `inference`, both default and `--no-default-features`.
   - The dependents I skipped and why:
     - Tests of local-voice-cascade: its dev-dependency builds audio-device's default `device` feature.
     - The stt-sherpa-onnx example, and the sherpa and whispercpp adapters.
   - The log.rs change, compared with `diff -w`, differs only in imports, the `mod emit` wrapper, a reflowed `control` signature and cfg lines; no logic changed. The `carry` gate matches its callers, duplex included.
   - inference-docs `crates.md` lists `duplex`, `llm`, `stt`, `tts`, which matches the manifest.

6. **Written outside the worktree:** `<scratch>` (combos.sh, summary.txt, 34 `*.log`, log.base.rs, log.head.rs) and `<scratch>`. `worktree discard-cache` has run on the tree (911 MiB).

```findings
[
  {"file":"CHANGELOG.md","line":11,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The Unreleased entry says inference-pipeline depends on inference-stt, inference-llm and inference-tts, but after this unit they are optional behind features stt, llm and tts, and the DuplexNode entry at :29 does not name feature duplex."},
  {"file":"crates/inference-pipeline/src/nodes/mod.rs","line":12,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"Neither the crate doc nor the nodes module doc says the model nodes are behind features, and under default features rustdoc cannot resolve the links to SttNode, ModelNode, AgentNode, TtsNode, SpeechNode and SttPayload."}
]
```
