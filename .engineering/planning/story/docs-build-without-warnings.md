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
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:17:27Z", actor: "human:operator", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-10T00:17:27Z", actor: "human:operator", revision: 6}
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

Rewritten 2026-11-10 from the merged change (`git diff ff0ba010 7b99b079`, 23 files). The defect was wider than the five sites scoped: the base failed in ten crates.

- **Gate:** `Taskfile.yml` (`task check` runs `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` after clippy), `AGENTS.md`
- **Crate docs linking feature-gated or absent items, now code:** `crates/inference/src/lib.rs`, `adapters/inference-aws/src/lib.rs`, `adapters/inference-deepgram/src/lib.rs`, `adapters/inference-elevenlabs/src/lib.rs`, `adapters/inference-openai/src/lib.rs`
- **Outer module docs that made the inner docs resolve in the parent's scope, dropped:** `crates/inference-testkit/src/{lib,cascade,duplex,graph,llm,model,stt,tts,tools}.rs`, `crates/inference-pipeline/src/nodes/mod.rs`, `crates/inference-llm/src/lib.rs`
- **Redundant link targets removed:** `adapters/inference-audio-device/src/capture.rs`, `adapters/inference-llama-server/src/lib.rs`, `crates/inference-runtime/src/queue.rs`, `crates/inference-pipeline/src/nodes/llm.rs`, `crates/inference-testkit/src/graph.rs`, `crates/inference-testkit/src/stt/conformance.rs`, `crates/inference-testkit/src/llm.rs`
- **Not reviewed by an adversary:** documentation text and one gate line only; the gate step is the check
