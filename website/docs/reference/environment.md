---
title: Environment
sidebar_position: 4
description: Every environment variable the adapters, examples and live tests read, what it does, and its default.
lede: Adapters and examples take their keys, checkpoints and servers from the environment or from options; nothing is looked up or downloaded on their behalf.
source: adapters/inference-openai (config.rs, credentials), adapters/inference-aws (nova_sonic), examples/realtime-voice, examples/local-voice-cascade (src/local.rs), examples/stt-sherpa-onnx, adapter tests
---

# Environment

No adapter looks a key up or downloads a checkpoint by itself. The examples read these variables;
the live tests read the same ones and are ignored without them.

## Hosted providers

| Variable | Read by | Effect | Default |
|---|---|---|---|
| `INFERENCE_OPENAI_KEY_SOURCE` | `realtime-voice`, `repo-podcast`, OpenAI live tests | `env` reads the OpenAI key from `OPENAI_API_KEY` instead of the keychain | the OS keychain, secret `openai-api-key` |
| `OPENAI_API_KEY` | as above | the OpenAI key, with `INFERENCE_OPENAI_KEY_SOURCE=env` | none |
| `INFERENCE_OPENAI_SECRET_NAME` | OpenAI live tests | another keychain secret name | `openai-api-key` |
| `INFERENCE_OPENAI_REALTIME_MODEL` | `inference-openai` | another OpenAI Realtime model | `gpt-realtime-2.1` |
| `AWS_REGION` | `inference-aws` | the Bedrock region for Nova Sonic; credentials come from the AWS default chain | none |
| `INFERENCE_NOVA_SONIC_MODEL` | `inference-aws` | another Nova Sonic model | `amazon.nova-2-sonic-v1:0` |
| `INFERENCE_COMPARE_BARGE_IN_WAV` | `realtime-voice`, example `compare` | a 16-bit PCM WAV file of speech sent over each answer as a barge-in | no barge-in |

The OpenAI key is sent once in the handshake and never logged by the adapter. The WebSocket library
below it logs the handshake at trace level: see
[Limitations](../status/limitations.md#hosted-adapters).

## Local models

| Variable | Option of `local-voice-cascade` `talk` | Effect | Default |
|---|---|---|---|
| `INFERENCE_CASCADE_STT` | `--stt` | the recogniser: `sherpa-onnx` or `whispercpp` | `sherpa-onnx` |
| `INFERENCE_SHERPA_ONNX_STT_DIR` | `--stt-dir` | the directory of the unpacked pinned sherpa-onnx checkpoint; also read by `stt-sherpa-onnx` and the sherpa-onnx tests | none |
| `INFERENCE_WHISPERCPP_STT_MODEL` | `--whispercpp-model` | the pinned `ggml-base.en.bin` | none |
| `INFERENCE_WHISPERCPP_STT_CLIP` | none | a speech clip for the whisper.cpp live tests | none |
| `INFERENCE_LLAMA_SERVER_URL` | `--server-url` | the base URL of a `llama-server` API on loopback; a host that is not loopback is refused | none |
| `INFERENCE_LLAMA_SERVER_MODEL` | `--server-model` | the model the server answers to, its `--alias` | none |

`talk` also takes `--max-output-tokens` (the most tokens of one answer, 64 by default), and `--input`
and `--output` to pick devices by name.

## Build

| Variable | Set by | Effect |
|---|---|---|
| `SHERPA_ONNX_LIB_DIR` | `.cargo/config.toml` | where the build finds the sherpa-onnx native library that `task sherpa-onnx-archive` unpacks under `target/sherpa-onnx` |
