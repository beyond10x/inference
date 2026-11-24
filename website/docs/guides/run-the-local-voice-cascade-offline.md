---
title: Run the voice cascade offline
sidebar_position: 2
description: Microphone, a local recogniser, a llama-server on loopback, a local synthesiser and the speaker, under offline_strict, with the pinned checkpoints and the exact commands.
lede: Speech recognition, the language model and speech synthesis run on your machine, and the run fails if a socket of the process connects anywhere but loopback.
source: examples/local-voice-cascade (examples/talk.rs, src/local.rs), adapters/inference-sherpa-onnx/src/stt.rs, adapters/inference-whispercpp/src/stt.rs, adapters/inference-llama-server, adapters/inference-sherpa-onnx-tts (README.md, src/lib.rs)
---

# Run the voice cascade offline

The `local-voice-cascade` example wires the microphone, a local recogniser, a `llama-server` on
loopback, a synthesiser and the speaker through commit gates, under the `offline_strict`
[deployment profile](../concepts/deployment-profiles.md).

:::warning[Requirements]

Linux x86_64 for sherpa-onnx (run `task sherpa-onnx-archive` once), a pinned recogniser
checkpoint, a `llama-server` binary and a GGUF model, the synthesiser's library and checkpoint
(step 3), a microphone and an output device. Use a headset: there is no echo cancellation.

:::

:::note[The synthesiser's library carries GPL-3.0 code]

The answer is spoken by `inference-sherpa-onnx-tts`. It runs on the sherpa-onnx native library
built with text-to-speech, which carries the phonemiser espeak-ng, GPL-3.0. The repository admits
that for this one adapter and distributes none of it: step 3 fetches it to your machine. What that
means is in [Limitations](../status/limitations.md#local-adapters).

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

### Or Ollama

The adapter talks to any OpenAI-compatible Chat Completions API on loopback. Ollama's is tested:
on 2026-11-12 the adapter's live test passed against Ollama 0.20.8 serving `qwen2.5:1.5b` at
`http://127.0.0.1:11434/v1`. Start it, fetch the model and send one request, since the first one
loads the model:

```sh
ollama serve
ollama pull qwen2.5:1.5b
curl -s http://127.0.0.1:11434/v1/chat/completions -H 'Content-Type: application/json' \
    -d '{"model":"qwen2.5:1.5b","messages":[{"role":"user","content":"Say hi."}],"max_tokens":8}'
```

Then set `INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:11434/v1` and
`INFERENCE_LLAMA_SERVER_MODEL=qwen2.5:1.5b` in step 4. The live test, run the same way:

```sh
INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:11434/v1 INFERENCE_LLAMA_SERVER_MODEL=qwen2.5:1.5b \
    cargo test -p inference-llama-server --features live --test live -- --ignored --nocapture
```

```text
running 1 test
server build: unknown: /props names no build_info
usage: Some(Some(Usage { input_tokens: Some(41), output_tokens: Some(21), cached_input_tokens: None, cache_creation_input_tokens: None, reasoning_output_tokens: None }))
deltas: 20, first after Some(13.561500523s), completed after 45.113786792s
text: "One, Two, Three, Four, Five, Six, Seven, Eight, Nine, Ten."
test a_running_server_streams_a_generation_and_refuses_input_during_output ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 45.12s
```

Ollama has no `/props`, so the server build is unknown. The times are those of a machine running
many builds at once, not of the model.

## 3. Get the synthesiser's library and checkpoint

```sh
task sherpa-onnx-tts
```

The task fetches two archives, checks each against its pinned sha256 before unpacking, and unpacks
them under `target/sherpa-onnx-tts`:

- `native/sherpa-onnx-v1.13.8-linux-x64-shared-lib/lib`: the sherpa-onnx native library built
  with text-to-speech;
- `checkpoint/kokoro-en-v0_19`: Kokoro-82M v0.19 for English, 24 kHz, with `model.onnx`,
  `voices.bin`, `tokens.txt` and `espeak-ng-data`. The adapter checks the three files and the
  tree of `espeak-ng-data` against their pins and refuses a directory that differs, naming the
  file.

The workspace builds and tests against the library without text-to-speech. A build links neither
library by path, so the one a process runs on is chosen when it starts: the command of step 4
starts the example on the library with text-to-speech through a cargo target runner. Started on the
other one, the example ends with a refusal that names the library.

## 4. Run the cascade

```sh
tts="$PWD/target/sherpa-onnx-tts"
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="env LD_LIBRARY_PATH=$tts/native/sherpa-onnx-v1.13.8-linux-x64-shared-lib/lib" \
INFERENCE_SHERPA_ONNX_STT_DIR=<checkpoint dir> \
INFERENCE_SHERPA_ONNX_TTS_DIR="$tts/checkpoint/kokoro-en-v0_19" \
INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:8080/v1 \
INFERENCE_LLAMA_SERVER_MODEL=qwen2.5-1.5b-instruct \
    cargo run -p local-voice-cascade --features local --example talk
```

The answer is printed and spoken. The synthesiser speaks each segment on its own and each delta of
the server is a segment, so an answer is spoken delta by delta, without the prosody of a sentence.

Each variable is also an option:

```sh
cargo run -q -p local-voice-cascade --features local --example talk -- --help
```

```text
Talk to the local voice cascade through a microphone and a speaker

Usage: talk [OPTIONS] --tts-dir <TTS_DIR> --server-url <SERVER_URL> --server-model <SERVER_MODEL>

Options:
      --stt <STT>
          The recogniser: sherpa-onnx or whispercpp [env: INFERENCE_CASCADE_STT=] [default: sherpa-onnx]
      --stt-dir <STT_DIR>
          The directory of the unpacked pinned checkpoint of the sherpa-onnx recogniser, for --stt sherpa-onnx. Read as given, so that an empty value of the recogniser not selected is ignored rather than refused [env: INFERENCE_SHERPA_ONNX_STT_DIR=]
      --whispercpp-model <WHISPERCPP_MODEL>
          The pinned checkpoint file of the whisper.cpp recogniser, for --stt whispercpp [env: INFERENCE_WHISPERCPP_STT_MODEL=]
      --tts-dir <TTS_DIR>
          The directory of the unpacked pinned checkpoint of the sherpa-onnx synthesiser [env: INFERENCE_SHERPA_ONNX_TTS_DIR=]
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
its live tests, ignored by default, read the same variables and need the same target runner:

```sh
cargo test -p local-voice-cascade --all-features -- --ignored
```

One of them needs neither a recogniser checkpoint nor a server: the mock recogniser and the mock
generator answer, and the synthesiser speaks into the fake host's recorded speaker. On 2026-11-24
it printed, on a 64-vCPU Xeon Platinum 8375C:

```text
spoken: ["how warm is it outside", "and will it rain"]; the speaker took 127 frames, 60791 samples of PcmS16le at 24000 Hz, 1 channel, 2532 ms; peak 15024 of 32767; receipts of [77, 50] frames; sockets outside loopback: None over 41 polls of the egress watch
```

## Next

- [Swap the recogniser by configuration](swap-the-recogniser-by-configuration.md)
- What each local adapter measured on the reference machine: [Adapters](../reference/adapters.md)
