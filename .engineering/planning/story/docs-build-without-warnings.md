---
format: aep.planning-md/3
id: story:docs-build-without-warnings
kind: story
status: active
title: The workspace's docs build without warnings and task check holds it
tags:
- from-finding
relations:
- informed_by: review-result:adversary-w25-sdk-payload-pass-1
- decomposes: epic:defects-2026-11-10
- serves: vision:inference
scope:
- confidence: inferred
  path: AGENTS.md
- confidence: cited
  path: Taskfile.yml
- confidence: inferred
  path: adapters/inference-aws/src/lib.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/lib.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/src/lib.rs
- confidence: inferred
  path: adapters/inference-openai/src/lib.rs
- confidence: cited
  path: crates/inference/src/lib.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:17:27Z", actor: "human:timo", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-10T00:17:27Z", actor: "human:timo", revision: 6}
---
## Context

`RUSTDOCFLAGS='-D warnings' cargo doc -p inference --no-deps` fails with
`unresolved link to 'transcribe'` at `crates/inference/src/lib.rs:3`: the crate doc links an item
that exists only with a feature. No gate builds the documentation, so broken intra-doc links in a
build without features go unnoticed. Found while checking the second SDK wave's payload docs.

## Planned scope

- `crates/inference/src/lib.rs`: the crate doc names feature-gated items as code, not as links.
- `Taskfile.yml`: `task check` builds the workspace's docs with `-D warnings` in a build without
  features.

## Acceptance

- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` exits 0, and `task check` runs it.

## Scope

Derived 2026-11-10 by `story-scoper` against `main` at `af93222a`. Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `Taskfile.yml`, the `check` task — cited
- **Files:** `crates/inference/src/lib.rs:3`, the `[`transcribe`]` link — cited
- **Files:** `Taskfile.yml` `check`: a new step `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps`, after `task: sherpa-onnx-archive`, cheapest right after the `cargo clippy` line; no `--exclude`s — inferred
- **Also likely:** `adapters/inference-aws/src/lib.rs:9` (`[`duplex`]`); `adapters/inference-deepgram/src/lib.rs:107,110,114`; `adapters/inference-elevenlabs/src/lib.rs:82,84,86,87`; `adapters/inference-openai/src/lib.rs:67-68` — inferred: each links items behind `duplex`, which only dev-dependencies turn on, and `cargo doc` does not apply dev-dependency features
- **Symbols:** `transcribe` — cited. No `fn transcribe` exists in `crates/inference` under any feature (`oneshot.rs` is a 4-line stub), so the link has no target; writing it as code is the fix until story:sdk-one-call lands
- **Not failing in a workspace build:** `crates/inference-llm/src/lib.rs:105`, `adapters/inference-sherpa-onnx/src/lib.rs:8`, `crates/inference-facade` `crate::cost` — inferred (feature unification turns them on)
- **Native crates:** sherpa-onnx cannot be documented in `--workspace` without the archive (`examples/stt-sherpa-onnx/Cargo.toml:12` turns `stt` on; `.cargo/config.toml` forces `SHERPA_ONNX_LIB_DIR`; `sherpa-onnx-sys-1.13.8/build.rs:117-124`), so the step comes after `task sherpa-onnx-archive`; whisper.cpp needs no archive and is compiled by `whisper-rs-sys` during `cargo doc` too — cited
- **Rejected alternative:** `DOCS_RS=1` would rebuild whisper.cpp on every `check` (`rerun-if-env-changed=DOCS_RS`) — inferred
- **Documents:** `AGENTS.md:49`, the comment on the `task check` line — inferred
- **Red:** `task sherpa-onnx-archive && RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --keep-going` — cited for `inference`, inferred for the four adapters
- **Green:** `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` exits 0, and `task check` runs it after `sherpa-onnx-archive` — cited (story Acceptance)
- **Confidence:** medium; the four adapter sites come from reading feature unification, not from a run
- **Would collide with:** any unit editing `Taskfile.yml` `check`, the crate-root `//!` docs of `inference`, `inference-aws`, `inference-deepgram`, `inference-elevenlabs` or `inference-openai`, or the `AGENTS.md` Commands block
- **Safety fact:** only `//!` text and one Taskfile step change; `inference-docs generate --check` reads manifests and records, not rustdoc — unproven

Defect confirmation on `main` at `af93222a`:
1. `crates/inference/src/lib.rs:3` links `[`transcribe`]`, and nothing in the crate defines it.
2. `Taskfile.yml` `check` (lines 8-18) has no `cargo doc` step.
3. The run is the implementor's red case (needs `task sherpa-onnx-archive` first).
