---
format: aep.planning-md/3
id: task:openai-test-required-features
kind: task
status: draft
title: inference-openai tests build under every feature set
revision: 1
---
## Context

`cargo clippy -p inference-openai --all-targets` without the `speech` feature fails with
`E0432 unresolved import inference_openai::speech` in `tests/security_blank_key_speech.rs`, which
has no `[[test]]` entry with `required-features = ["speech"]`. The workspace gate builds with
`--all-features`, so CI does not see it; a one-crate gate with fewer features does (found during
story:async-speech-handles, 2026-11-13).

## Planned scope

- `adapters/inference-openai/Cargo.toml` gives that test its `required-features`.
- Any other test of the crate that imports a feature-gated module gets the same.

## Acceptance

- `cargo clippy -p inference-openai --all-targets -- -D warnings` passes with no features, with
  `duplex` alone, and with `duplex,gpt-live`.
