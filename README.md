# inference

**Documentation: https://beyond10x.github.io/inference/**

Rust contracts, adapters, pipelines and a runtime for AI inference: speech-to-text, language
models and text-to-speech as interchangeable typed nodes, composed into validated streaming graphs.
Self-hosted and open-source first; hosted providers are opt-in adapters.

**Status: released as 0.6.0 (see [CHANGELOG.md](CHANGELOG.md)).** The contracts, pipelines,
runtime, session facade and native speech contract are implemented under `crates/`; hosted and
local adapters are under `adapters/`, runnable examples under `examples/`. The design is [docs/design/inference.md](docs/design/inference.md) (v0.2, 2026-10-16); its review is
[docs/design/review-2026-10-18.md](docs/design/review-2026-10-18.md).
The plan is the AEP planning store under `.engineering/planning/`.

Build with `cargo build --workspace` (Rust stable, 1.98 or later). Check with `task check`
([go-task](https://taskfile.dev)), which also needs the `ess` and `aep` command-line tools.

Apache-2.0.
