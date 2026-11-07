---
format: aep.planning-md/3
id: review-result:adversary-w24-sdk-crate-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the inference crate
relations:
- reviews: story:sdk-crate
revision: 1
---
unit: W24-sdk-crate story:sdk-crate, tree <scratch> findings cover 0e02954 plus adversary commit 04c8dd8
verdict: NEEDS-CHANGE
cases: executed 10→17, red 1
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 directory, <scratch> (story dumps, probe.sh, suite.sh, logs, tree outputs, commit message file msg.QnSx9d)
needs-coordinator: three acceptance lines were never run, by me or by the implementor, because they build `audio-device`: `cargo check -p inference --all-features`, `cargo test -p inference --all-features --no-run` and `cargo check -p inference --no-default-features --features audio-device`. The fix for finding 1 is in `crates/inference-docs/src/crates.rs`, which is outside this story's scope. The implementor already wrote it as a patch: W24-sdk-crate/inference-docs-proc-macro.patch.

**1. Diff stat** (0e02954..HEAD; test files only, and the uncommitted Cargo.lock is untouched):
```
 crates/inference/tests/adversary_w24_error.rs     | 189 +++
 crates/inference/tests/adversary_w24_reference.rs |  40 +++
```

**2. Cases** (commit 04c8dd8, authored and committed by b10x-bot[bot]; scan-text: "3 file(s) carry no private rule")
- `adversary_w24_reference.rs`: the generated crate reference must not call `inference-macros` "Examples only.". It is **red**. First run was `cargo-run.sh test -p inference --no-default-features --test adversary_w24_reference --test adversary_w24_error`, which printed:
  `panicked at crates/inference/tests/adversary_w24_reference.rs:34:9: the reference row of inference-macros, a proc-macro library every SDK user compiles, says it holds examples only: | [`inference-macros`](…) | none | Examples only. The attribute macros of the inference SDK: …` (`test result: FAILED. 0 passed; 1 failed`).
- `adversary_w24_error.rs` has 6 cases, all **green** (`6 passed`):
  - every Locality×ProfileKind pair (16) is written as its serde name in `ProfileDenies`;
  - `RoleUnbound` names `INFERENCE_<ROLE>_ADAPTER` and `[<role>]` for stt, llm, tts and speech;
  - `Error` is Send + Sync + 'static and boxes as a std error;
  - `report` of `ProfileDenies` writes exactly 2 lines, the second being the `LocalityDenied`;
  - `Provider` returns its source and `report` writes it. Nothing tested this before; a mutant making `Provider` return `None` from `source()` survived the unit's suite;
  - the paths later stories name all resolve: pipeline `Catalog`/`Declaration`/`PipelineId`, `ProfileId`, `AdapterId`, `Capability`, `tools::{Tool, ToolExecutor, SideEffectClass}`, `text::Span`, `facade::live::LiveSessions`, `llm::llm_core::Usage`, and `__private` `serde_json`/`tokio`/`exit`.
- One deviation from the procedure: I added the `Provider` case while the first run was still compiling, and it ran in that first run.

**3. Suite, run after the cases**
- `test -p inference --features audio-host --lib --test adversary_w24_error --test adversary_w24_reference --no-fail-fast`: rc=101. lib `9 passed`, error `6 passed`, reference `0 passed; 1 failed`.
- `test -p inference --features audio-host --doc`: rc=0, `1 passed`. The prelude doc test passes with `&dyn AudioHost` and `FakeHost`.
- `clippy -p inference --features audio-host --test adversary_w24_error --test adversary_w24_reference -- -D warnings`: rc=0.
- Cargo time was about 3.7 min (sum of the wrapper's `secs`). Wall time was not measured and was mostly waiting for a build slot. `target/` peaked at 859M; `worktree discard-cache` then cleared 858 MiB.

**4. Findings**
- **Finding 1.** `website/docs/reference/crates.md:26` (generator: `crates/inference-docs/src/crates.rs:114`).
  - The published reference calls the proc-macro crate behind `#[inference::tool]` and `#[inference::main]` "Examples only.".
  - Cause: the generator counts only `lib` and `bin` targets, and this crate's only target is `proc-macro`.
  - Reached by: `task docs-generate`, and by the published site.
  - NEEDS-CHANGE, introduced (no package reached that path before this unit).
  - Fix: count the `proc-macro` kind as a library at crates.rs:114, then regenerate.
- **Finding 2.** `crates/inference/Cargo.toml:21` and the matching story acceptance line.
  - `cargo tree -p inference -e features --features testing`, as the acceptance writes it, prints 0 `inference feature` lines, because cargo does not show the root package's own features unless the tree is inverted. The line therefore cannot show `audio-host`, and its "no `audio-device`" half passes on empty output. The implementor's tree.log "testing features:" section is empty.
  - With `-i inference`, the tree lists `inference feature "audio-host"` and `"testing"`, and no `audio-device`, so the property itself holds.
  - CONFIRMED, introduced.

**5. Attacked and not broken**
- Each feature alone with `--no-default-features` checks rc=0: none, openai, openai-live, aws, llama-server, cost, audio-host, testing. Not tried: sherpa-onnx (needs the native archive), whispercpp (cmake build), audio-device (device rule).
- Normal dependency tree: no crate under `adapters/` with no default features (153 lines) or with `cost`. `testing` adds only `inference-audio-device` and no cpal. `audio-device` resolves cpal 0.18.2.
- The nine `Error` variants and their fields match the §9 table in docs/design/sdk.md, and each of the six fixable variants' `Display` names its fix.
- I read the Planned scope of every later story (sdk-stack, sdk-payload, sdk-tool-macro, sdk-recipes, sdk-agent, sdk-one-call, sdk-usage-cost, sdk-testing, sdk-examples). None needs a new dependency, feature or re-export in lib.rs, prelude.rs or Cargo.toml:
  - `PriceBook::parse_json` is reachable through the facade, and `llm_core` through `inference::llm`.
  - One soft spot: `__private` has no `futures_util`, so `assert_events!` in sdk-testing has to reach `.next()` through a hidden item in testing.rs.
- The `openai` feature turning on only `inference-openai/duplex` is right. The SDK's `speech` role is `NativeSpeechModel` (`duplex`, plus `live_duplex` under `gpt-live`). inference-openai's `speech` feature is a TTS synthesiser that no story registers.

**6. Paths written outside the worktree:** <scratch> (all files inside it)

**7. Findings block**
```findings
[
  {"file":"website/docs/reference/crates.md","line":26,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The generated crate reference calls the proc-macro crate inference-macros 'Examples only.' because crates/inference-docs/src/crates.rs:114 counts only lib and bin targets as libraries."},
  {"file":"crates/inference/Cargo.toml","line":21,"category":"acceptance","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The acceptance command 'cargo tree -p inference -e features --features testing' prints no inference feature line at all, so its audio-host/no-audio-device check is vacuous; with -i inference it shows audio-host and testing and no audio-device."}
]
```
