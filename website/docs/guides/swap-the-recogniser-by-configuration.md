---
title: Swap the recogniser by configuration
sidebar_position: 3
description: Bind the cascade's speech recogniser to sherpa-onnx or whisper.cpp with one variable, and see what changes and what is refused before anything opens.
lede: One value, INFERENCE_CASCADE_STT, decides which recogniser the cascade's stt node binds; the declaration, the nodes and the tests stay the same.
source: examples/local-voice-cascade (src/local.rs, examples/talk.rs, tests/adversary1_stt_swap.rs, tests/local_adapters.rs), CHANGELOG.md 0.7.0
---

# Swap the recogniser by configuration

Since 0.7.0 the local voice cascade binds its `stt` node to the recogniser that
`INFERENCE_CASCADE_STT`, or `talk --stt`, selects:

| Value | Recogniser | Checkpoint from | What you see |
|---|---|---|---|
| `sherpa-onnx` (the default) | sherpa-onnx's streaming recogniser | `INFERENCE_SHERPA_ONNX_STT_DIR` or `--stt-dir` | hypotheses while you speak, then commits |
| `whispercpp` | whisper.cpp's batch recogniser | `INFERENCE_WHISPERCPP_STT_MODEL` or `--whispercpp-model` | one committed span per turn, after the pause that ends it |

Both adapter crates are dependencies of the `local` feature. Only the selected recogniser's
checkpoint is needed. The `stt` node requires no capability, so the batch recogniser, which claims
neither incremental input nor output streaming, plans there. The cascade's effective capabilities
are what all three of its adapters claim: `output_streaming` with sherpa-onnx, none with
whisper.cpp.

## Run it under whisper.cpp

With the server from [Run the voice cascade offline](run-the-local-voice-cascade-offline.md):

```sh
INFERENCE_CASCADE_STT=whispercpp \
INFERENCE_WHISPERCPP_STT_MODEL=<path to ggml-base.en.bin> \
INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:8080/v1 \
INFERENCE_LLAMA_SERVER_MODEL=qwen2.5-1.5b-instruct \
    cargo run -p local-voice-cascade --features local --example talk
```

The same live tests run under either value, once per value:

```sh
INFERENCE_CASCADE_STT=whispercpp cargo test -p local-voice-cascade --all-features -- --ignored
```

## What is refused before anything opens

A value that names no recogniser is refused, naming it:

```sh
cargo run -q -p local-voice-cascade --features local --example talk -- --stt whisper --server-url http://127.0.0.1:8080/v1 --server-model qwen2.5-1.5b-instruct
```

```text
error: invalid value 'whisper' for '--stt <STT>': INFERENCE_CASCADE_STT is "whisper", which names no recogniser: one of ["sherpa-onnx", "whispercpp"]

For more information, try '--help'.
```

A server off loopback is refused before any checkpoint is read:

```sh
cargo run -q -p local-voice-cascade --features local --example talk -- --stt whispercpp --whispercpp-model ggml-base.en.bin --server-url http://192.0.2.1:8080/v1 --server-model qwen2.5-1.5b-instruct
```

```text
error: http://192.0.2.1:8080/v1 is not an http URL whose host is loopback: the local cascade's profile denies egress
```

The refusals at the edges of the value (empty, wrong case, padded, an alias, a list) and before
the checkpoint is read are held by
[`examples/local-voice-cascade/tests/adversary1_stt_swap.rs`](https://github.com/beyond10x/inference/blob/main/examples/local-voice-cascade/tests/adversary1_stt_swap.rs).
