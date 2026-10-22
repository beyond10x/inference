---
title: Examples
sidebar_label: All examples
sidebar_position: 1
description: The five runnable example packages, what each shows, what it needs and where its guide is.
lede: Five example packages build and test with the workspace; the ones that need a model, a server or a key are ignored without it.
source: examples/ (stt-only, stt-sherpa-onnx, realtime-voice, local-voice-cascade, repo-podcast), CHANGELOG.md
---

# Examples

The example packages under `examples/` are built and tested with the workspace and never published.
Tests that need a model, a server or a key read it from an environment variable and are ignored
without it; the rest run on the testkit's mocks and a fake audio host.

| Package | Shows | Needs | Since | Read |
|---|---|---|---|---|
| `stt-only` | one contract and a mock, nothing else | nothing | 0.4.0 | [Getting started](../getting-started.md) |
| `stt-sherpa-onnx` | one contract and one local adapter, on a WAV file | the sherpa-onnx checkpoint | 0.6.0 | [Transcribe a WAV file](../guides/transcribe-a-wav-file.md) |
| `realtime-voice`, example `talk` | a spoken conversation with a hosted native speech model, chosen by `--provider` | a key or credentials, microphone, speaker | 0.5.0 | [Talk to a hosted voice model](../guides/talk-to-a-hosted-voice-model.md) |
| `realtime-voice`, example `compare` | the same recorded question through each provider: latency and usage | keys or credentials | 0.6.0 | [Compare voice providers](../guides/compare-voice-providers.md) |
| `local-voice-cascade` | STT, LLM and TTS through commit gates, offline | with feature `local`: a checkpoint, a loopback llama-server, microphone, speaker | 0.6.0 | [Local voice cascade](local-voice-cascade.md) |
| `repo-podcast` | two native speech models discuss a repository in a terminal UI, with token and cost counters | the Claude Code CLI, an OpenAI key, speaker | 0.7.0 | [Repository podcast](repo-podcast.md) |

The first runs anywhere:

```sh
cargo run -p stt-only --example stt_only
```

```text
committed: hello there
committed: how are you
```
