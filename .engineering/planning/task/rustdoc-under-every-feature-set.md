---
format: aep.planning-md/3
id: task:rustdoc-under-every-feature-set
kind: task
status: draft
title: Rustdoc has no warnings under the feature sets the gate skips
revision: 1
---
## Context

`cargo doc` with `RUSTDOCFLAGS="-D warnings"` fails on `main` for two feature sets the workspace
gate does not build alone (found during story:async-speech-handles, 2026-11-13):

- `inference-openai` with `speech`: `src/speech/mod.rs:37`, "redundant explicit link target".
- `inference-testkit` without its `run` feature: unresolved `inference_runtime` links in
  `src/graph.rs`.

## Planned scope

- Fix both links so they resolve or drop the redundant target under every feature set.

## Acceptance

- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` passes for `inference-openai --features speech`
  and for `inference-testkit --no-default-features`.
