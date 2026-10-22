---
title: Run the voice cascade offline
sidebar_position: 2
description: Microphone, a local recogniser, a llama-server on loopback and the speaker, under offline_strict, with the pinned checkpoints and the exact server command.
lede: Speech recognition and the language model run on your machine, and the run fails if a socket of the process connects anywhere but loopback.
source: examples/local-voice-cascade (examples/talk.rs, src/local.rs), adapters/inference-sherpa-onnx/src/stt.rs, adapters/inference-whispercpp/src/stt.rs, adapters/inference-llama-server
---

# Run the voice cascade offline

The `local-voice-cascade` example wires the microphone, a local recogniser, a `llama-server` on
loopback, a synthesiser and the speaker through commit gates, under the `offline_strict`
[deployment profile](../concepts/deployment-profiles.md).

:::warning[Requirements]

Linux x86_64 for sherpa-onnx (run `task sherpa-onnx-archive` once), a pinned recogniser
checkpoint, a `llama-server` binary and a GGUF model, a microphone and an output device. Use a
headset: there is no echo cancellation.

:::

:::note[No local speech yet]

A local TTS adapter is planned. Until it exists, the cascade binds the mock synthesiser: the answer
is printed, the speaker plays 10 ms of silence per word, and speaking over it stops that playback.
Why it waits is in [Limitations](../status/limitations.md#no-local-text-to-speech).

:::

## 1. Get a recogniser checkpoint

Each checkpoint is pinned by sha256; the adapter checks every file and refuses a mismatch, naming
the file.

**sherpa-onnx**, the default: the checkpoint `sherpa-onnx-streaming-zipformer-en-2023-06-26` at
revision `672fbf1b30579d6585301139bb363f42a0ad4a24`, from

```text
https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26
```

into one directory holding these four files:

- `encoder-epoch-99-avg-1-chunk-16-left-128.onnx`
- `decoder-epoch-99-avg-1-chunk-16-left-128.onnx`
- `joiner-epoch-99-avg-1-chunk-16-left-128.onnx`
- `tokens.txt`

**whisper.cpp**: Whisper base.en converted to ggml, the one file `ggml-base.en.bin` of
147,964,211 bytes, from

```text
https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin
```

## 2. Start llama-server on loopback

Get a prebuilt `llama-server` release (MIT) and `qwen2.5-1.5b-instruct-q4_k_m.gguf` from
`https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF`, and start it:

```sh
llama-server --host 127.0.0.1 --port 8080 -m qwen2.5-1.5b-instruct-q4_k_m.gguf \
    --alias qwen2.5-1.5b-instruct -c 4096 -np 1 -ngl 0
```

## 3. Run the cascade

```sh
INFERENCE_SHERPA_ONNX_STT_DIR=<checkpoint dir> \
INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:8080/v1 \
INFERENCE_LLAMA_SERVER_MODEL=qwen2.5-1.5b-instruct \
    cargo run -p local-voice-cascade --features local --example talk
```

Each variable is also an option:

```sh
cargo run -q -p local-voice-cascade --features local --example talk -- --help
```

```text
Talk to the local voice cascade through a microphone and a speaker

Usage: talk [OPTIONS] --server-url <SERVER_URL> --server-model <SERVER_MODEL>

Options:
      --stt <STT>
          The recogniser: sherpa-onnx or whispercpp [env: INFERENCE_CASCADE_STT=] [default: sherpa-onnx]
      --stt-dir <STT_DIR>
          The directory of the unpacked pinned checkpoint of the sherpa-onnx recogniser, for --stt sherpa-onnx. Read as given, so that an empty value of the recogniser not selected is ignored rather than refused [env: INFERENCE_SHERPA_ONNX_STT_DIR=]
      --whispercpp-model <WHISPERCPP_MODEL>
          The pinned checkpoint file of the whisper.cpp recogniser, for --stt whispercpp [env: INFERENCE_WHISPERCPP_STT_MODEL=]
      --server-url <SERVER_URL>
          The base URL of the llama-server's API, on loopback [env: INFERENCE_LLAMA_SERVER_URL=]
      --server-model <SERVER_MODEL>
          The model the server answers to, its --alias [env: INFERENCE_LLAMA_SERVER_MODEL=]
      --max-output-tokens <MAX_OUTPUT_TOKENS>
          The most tokens of one answer [default: 64]
      --input <INPUT>
          The input device by name; the default input otherwise
      --output <OUTPUT>
          The output device by name; the default output otherwise
  -h, --help
          Print help
```

A server URL whose host is not loopback is refused before anything opens, and the run fails if a
socket of the process connects anywhere but loopback. Each turn prints the time from committed
input to first playable audio, and each interruption the time to playback stop. Ctrl-C ends the
conversation.

No conversation output is shown here: a run needs the checkpoints, the server and a microphone.
The same cascade runs on the testkit mocks and a fake audio host in
[`examples/local-voice-cascade/tests`](https://github.com/beyond10x/inference/tree/main/examples/local-voice-cascade/tests);
its live tests, ignored by default, read the same variables:

```sh
cargo test -p local-voice-cascade --all-features -- --ignored
```

## Next

- [Swap the recogniser by configuration](swap-the-recogniser-by-configuration.md)
- What each local adapter measured on the reference machine: [Adapters](../reference/adapters.md)
