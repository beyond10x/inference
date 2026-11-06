---
format: aep.planning-md/3
id: review-result:adversary-w23-descriptor-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the adapter descriptor carrier
relations:
- reviews: story:adapter-descriptor-carrier
revision: 1
---
unit: W23-descriptor story:adapter-descriptor-carrier, unit head 1facc91 (base 219efe2), adversary commit 8227d54 in tree inference-0-23-0-descriptor
verdict: CONFIRMED
cases: executed 66→133, red 1
origin: introduced 1 / pre-existing 1 / undecided 0
wrote-outside-worktree: 7 paths (part 6)
needs-coordinator: the CHANGELOG `Breaking` line in Acceptance is not in the tree yet. It is your file under W23.md.

**1. Diff stat** (`git --no-pager diff --stat 1facc91 HEAD`). Only one test file changed, so the scope rule held.
```
 .../tests/adversary_w23_descriptor_view.rs         | 356 +++++++++++++++++++++
 1 file changed, 356 insertions(+)
```

**2. Cases added**, in `crates/inference-testkit/tests/adversary_w23_descriptor_view.rs`. Each test has a 28 s deadline. The tests pair a witness scenario with a wrapper target that rewrites the `descriptor` of rows in `inference.model.Adapters`, and they run only the scenarios that read that view. The witness declares two voices, two codecs, and two rate limits where `limit` differs from `window_ms`.
- `adversary_w23_the_witness_rejects_every_rewrite_and_the_suite_rejects_a_dropped_descriptor`: **green**. The unmutated target passes. The witness rejects all three rewrites: Default, FirstOnly and SwapRate. The synthesized `DeclareAdapter/outcome/declared` rejects Default, so a mutant that drops the descriptor is caught.
- `adversary_w23_the_repository_scenarios_reject_a_truncated_or_swapped_descriptor`: **red**. First run, `cargo-run.sh test -p inference-testkit --features run --test adversary_w23_descriptor_view`, rc=101 secs=85:
```
rewrites of the Adapters view's descriptor that the witness rejects and every scenario of the repository passes: [FirstOnly, SwapRate]; the synthesized declaration sends one voice, one codec and one rate limit whose limit equals its window_ms, and no authored scenario reads the view
test result: FAILED. 66 passed; 1 failed; ... finished in 5.11s
```

**3. Suite run, after the cases existed.** Command: `cargo-run.sh test -p inference-testkit --features run --no-fail-fast --test conform --test adversary_w23_descriptor_view`, rc=101 secs=19.
- `adversary_w23_descriptor_view`: 66 passed, 1 failed.
- `conform`: 66 passed, 0 failed.

The "before" count of 66 is the implementor's `conform` count.

Clippy on the crates the implementor did not lint, rc=0 secs=70: testkit (run, stt, llm, tts, duplex), duplex, pipeline, llm, runtime, aws, cartesia, deepgram, deepslate, elevenlabs, openai (duplex, gpt-live, speech), phonic, llama-server (tools), all with `--all-targets -- -D warnings`. `cargo fmt --check` was clean and scan-text was clean.

Totals: cargo 3.0 min, wall about 25 min, peak `target/` 1.5G. `discard-cache` was run.

**4. Findings**
- **F1** (`crates/inference-testkit/tests/target/model.rs:364`, `ess/scenarios`): CONFIRMED / introduced, warning.
  - **What was measured:** the red case above. The only scenario that checks the descriptor in the view sends one element per list and `limit == window_ms` (1/1, 138600/138600, and so on). No authored scenario reads `Adapters`, so a target that truncates lists or swaps rate-limit fields passes `task check`.
  - **What reaches it:** no current code path truncates or swaps. The gap is in the suite's ability to tell these apart, so this is the Acceptance "synthesized suite covers the field" met in form only.
  - **Fix:** add an authored `ess/scenarios` scenario like this test's witness, which asserts the `Adapters` row whole.
- **F2** (`crates/inference-core/src/model.rs:496`): CONFIRMED / pre-existing, note.
  - `AdapterDescriptor` uses `#[serde(default)]` on `voices`, `codecs` and `rate_limits`. `model.yaml` declares them as required `List`s, and ess refuses `descriptor: {}` (ESS-AUTHOR-014 "requires `voices`… `codecs`… `rate_limits`", probe in scratch).
  - With this unit, `DeclareAdapter` accepts on the wire an input the spec refuses. The unit's own edits to `adversary2_identifier_spelling.rs` and `adversary_model_wire.rs` send `"descriptor": {}`.
  - **What reaches it:** a hand-written JSON input. Nothing in the repository sends one except those two tests. The base has the same `serde(default)` lines.

**5. Attacked, not broken**
- A mutant that drops the descriptor or resets it to default is caught by the synthesized scenario.
- Null vs absent optional fields: the ess runner treats them as equal below top level (`runner.rs:3557`). A partial-descriptor scenario passes the interpreted target.
- Spec and code agree on field names, types and optionality on the input, the entity and the view. `AdapterDeclared` carries no descriptor, matching the spec.
- `typed()` reads the descriptor through serde with `deny_unknown_fields`. `RateLimit` refuses missing fields.
- Re-declarations built from an existing adapter keep its descriptor (`llm.rs`, `adversary1_stt_swap.rs`).
- No claims record or capability file names voices or codecs, so `AdapterDescriptor::default()` everywhere meets Acceptance line 3.
- Clippy is clean on every unlinted crate except sherpa-onnx, whispercpp, audio-device and the examples, which I did not lint.

**6. Paths written outside the worktree**
- `<scratch>` holds:
  - `scenarios/` (copies of `ess/scenarios` plus `adversary-descriptor-partial.yaml` and `adversary-descriptor-empty-map.yaml`)
  - `suite.json`, `suite2.json`, `report.json`, `interp.txt`
  - `red-view.txt`, `suite-run.txt`, `clippy.txt`
- The commit message file `msg.NiIXcB` was deleted.

```findings
[{"file":"crates/inference-testkit/tests/target/model.rs","line":364,"category":"mutant","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"the repository's scenarios pass an Adapters view that truncates descriptor lists to one element or swaps a rate limit's limit and window_ms, because the only view check sends single-element lists and limit equal to window_ms and no authored scenario reads the view"},
 {"file":"crates/inference-core/src/model.rs","line":496,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"AdapterDescriptor defaults voices, codecs and rate_limits when absent while model.yaml declares them required, so DeclareAdapter now accepts a descriptor {} that the specification refuses"}]
```
