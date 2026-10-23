# inference

**Documentation: <https://beyond10x.github.io/inference/>**, starting at
[Getting started](https://beyond10x.github.io/inference/docs/getting-started).

Rust contracts, adapters, pipelines and a runtime for AI inference: speech-to-text, language
models and text-to-speech as interchangeable typed nodes, composed into validated streaming graphs.
Self-hosted and open-source first; hosted providers are opt-in adapters.

**Status: released as 0.8.0 (see [CHANGELOG.md](CHANGELOG.md)).** The contracts, pipelines,
runtime, session facade and native speech contract are implemented under `crates/`; hosted and
local adapters are under `adapters/`, runnable examples under `examples/`. The
[status page](https://beyond10x.github.io/inference/docs/status) lists what has shipped, what is on
`main` for the next release and what is planned; [Crates](https://beyond10x.github.io/inference/docs/reference/crates)
lists every crate and its features.

Build with Rust stable, 1.98 or later; the whisper.cpp adapter also needs `cmake`, a C++ compiler
and `libclang`. The sherpa-onnx adapter links a native library that the build does not download;
fetch and check it first:

```sh
task sherpa-onnx-archive
cargo build --workspace
```

Check with `task check` ([go-task](https://taskfile.dev)), which also needs the `ess` and `aep`
command-line tools.

Apache-2.0.
