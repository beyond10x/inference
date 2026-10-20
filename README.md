# inference

Rust contracts, adapters, pipelines and a runtime for AI inference: speech-to-text, language
models and text-to-speech as interchangeable typed nodes, composed into validated streaming graphs.
Self-hosted and open-source first; hosted providers are opt-in adapters.

**Status: design draft; the first crate, `inference-core`, holds the model vocabulary.** The
design is [docs/design/inference.md](docs/design/inference.md) (v0.2, 2026-10-16); its review is
[docs/design/review-2026-10-18.md](docs/design/review-2026-10-18.md).
The plan is the AEP planning store under `.engineering/planning/`.

Build with `cargo build --workspace` (Rust stable, 1.98 or later). Check with `task check`
([go-task](https://taskfile.dev)), which also needs the `ess` and `aep` command-line tools.

Apache-2.0.
