# Quick start

## Requirements

| For | You need |
|---|---|
| building | Rust stable, 1.98 or later |
| `task check` | [go-task](https://taskfile.dev), and the `ess` and `aep` command-line tools |
| the sherpa-onnx adapter <span class="st st-rel">0.6.0</span> | Linux x86_64: the task below fetches the pinned `linux-x64` native archive |
| the whisper.cpp adapter <span class="st st-rel">0.6.0</span> | `cmake`, a C++ compiler and `libclang`; whisper.cpp is compiled from source, CPU only |
| the voice examples | a microphone and an output device; use a headset, there is no echo cancellation |

## Build and check

```sh
git clone https://github.com/beyond10x/inference
cd inference
task sherpa-onnx-archive
cargo build --workspace
task check
```

`task sherpa-onnx-archive` fetches the sherpa-onnx native library built without TTS, checks it
against its pinned sha256 and unpacks it under `target/sherpa-onnx`, where `.cargo/config.toml`
points `SHERPA_ONNX_LIB_DIR`. A build without it fails rather than downloading anything.
`task check` runs that task first, then specification and planning-store validation, `cargo fmt`,
`cargo clippy -D warnings` and `cargo test --workspace --all-features`.

## 1. Speech recognition alone

```sh
cargo run -p stt-only --example stt_only
```

Feeds a fixture of audio frames to the mock recogniser through the STT contract and prints each
committed span. Its dependency tree is `inference-stt` and the testkit's `stt` feature: no
runtime, no model.

## 2. Talk to a hosted realtime model <span class="st st-rel">0.5.0</span>

Store the OpenAI key once in the OS keychain, with the `secretsctl` command of the `secrets`
library:

```sh
secretsctl put openai-api-key
cargo run -p realtime-voice --example talk -- --provider openai
```

Speak, interrupt the model mid-answer, ask for the time (a demo tool). Enter ends the conversation
and prints the session's usage totals and duration; Ctrl-C ends it without them.

| Variable | Effect |
|---|---|
| `INFERENCE_OPENAI_KEY_SOURCE=env` | read the key from `OPENAI_API_KEY` instead of the keychain |
| `INFERENCE_OPENAI_REALTIME_MODEL` | another model than the default `gpt-realtime-2.1` |
| `AWS_REGION` | the region for `--provider nova`; credentials come from the AWS default chain |
| `INFERENCE_NOVA_SONIC_MODEL` | another model than the default `amazon.nova-2-sonic-v1:0` |

The key is never printed. `--prices <file>` adds a cost estimate from a price table you supply
(see [Usage and cost](usage-and-cost.md)).

## 3. The local voice cascade, offline <span class="st st-rel">0.6.0</span>

Microphone, sherpa-onnx speech recognition, a `llama-server` on loopback, and the speaker.

1. Download the sherpa-onnx checkpoint `sherpa-onnx-streaming-zipformer-en-2023-06-26` from
   `https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26` (revision
   `672fbf1b`) into one directory: `encoder-epoch-99-avg-1-chunk-16-left-128.onnx`,
   `decoder-epoch-99-avg-1-chunk-16-left-128.onnx`, `joiner-epoch-99-avg-1-chunk-16-left-128.onnx`
   and `tokens.txt`. The adapter checks each file's sha256 and refuses a mismatch, naming the file.
2. Get a prebuilt `llama-server` release (MIT) and `qwen2.5-1.5b-instruct-q4_k_m.gguf` from
   `https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF`, and start it on loopback:

   ```sh
   llama-server --host 127.0.0.1 --port 8080 -m qwen2.5-1.5b-instruct-q4_k_m.gguf \
       --alias qwen2.5-1.5b-instruct -c 4096 -np 1 -ngl 0
   ```

3. Run the cascade:

   ```sh
   INFERENCE_SHERPA_ONNX_STT_DIR=<checkpoint dir> \
   INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:8080/v1 \
   INFERENCE_LLAMA_SERVER_MODEL=qwen2.5-1.5b-instruct \
       cargo run -p local-voice-cascade --features local --example talk
   ```

Each variable is also an option (`--stt-dir`, `--server-url`, `--server-model`); `--input` and
`--output` pick devices by name. A server URL whose host is not loopback is refused before anything
opens, and the run fails if a socket of the process connects anywhere but loopback.

<div class="note">
A local TTS adapter is <span class="st st-plan">planned</span>. Until it exists, the cascade binds
the mock synthesiser: the answer is printed, the speaker plays 10 ms of silence per word, and
speaking over it stops that playback. Each turn prints the time from committed input to first
playable audio.
</div>
