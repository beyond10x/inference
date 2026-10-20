# Examples

Example packages under `examples/` are built and tested with the workspace and never published.
Tests that need a model, a server or a key read it from an environment variable and are ignored
without it; the rest run on the testkit's mocks and a fake audio host.

| Package | Shows | Status |
|---|---|---|
| [`stt-only`](#stt-only) | one contract and a mock, nothing else | <span class="st st-rel">0.5.0</span> |
| [`realtime-voice`](#realtime-voice) | a spoken conversation with a hosted native speech model | <span class="st st-rel">0.5.0</span> |
| [`stt-sherpa-onnx`](#stt-sherpa-onnx) | one contract and one local adapter, on a WAV file | <span class="st st-rel">0.6.0</span> |
| [`local-voice-cascade`](#local-voice-cascade) | STT, LLM and TTS through commit gates, offline | <span class="st st-rel">0.6.0</span> |

## stt-only

```sh
cargo run -p stt-only --example stt_only
```

Feeds a fixture of audio frames to the testkit's mock recogniser through the STT contract and
prints each committed span. Depends on `inference-stt` and the testkit's `stt` feature only: no
runtime in its dependency tree.

## realtime-voice

```sh
cargo run -p realtime-voice --example talk -- --provider openai
cargo run -p realtime-voice --example talk -- --provider openai --prices prices.json
INFERENCE_OPENAI_KEY_SOURCE=env OPENAI_API_KEY=... cargo run -p realtime-voice --example talk -- --provider openai
AWS_REGION=<region> cargo run -p realtime-voice --example talk -- --provider nova
```

The default microphone feeds an `inference-duplex` session; its audio plays on the default output.
Both transcripts are printed. When the user starts speaking, the speaker stops at once and the
model's answer is truncated at the played position. One demo tool tells the local time. The same
`Conversation` code runs either provider: `--provider` changes the binding, not the application.
Usage per response and session totals are printed <span class="st st-rel">0.6.0</span>.

Use a headset. Enter ends with totals; Ctrl-C ends without them.

## stt-sherpa-onnx

```sh
INFERENCE_SHERPA_ONNX_STT_DIR=<checkpoint dir> cargo run -p stt-sherpa-onnx --example transcribe -- <file.wav>
```

Reads a WAV file (16-bit PCM, 32-bit float, G.711 mu-law or A-law, any rate and channel count),
streams it in 20 ms frames through the sherpa-onnx recogniser and prints each committed span.
Depends on `inference-stt` and the adapter's `stt` feature only. The checkpoint is described in
[Quick start](quick-start.md#3-the-local-voice-cascade-offline-060).

## local-voice-cascade

```sh
INFERENCE_SHERPA_ONNX_STT_DIR=<checkpoint dir> \
INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:8080/v1 \
INFERENCE_LLAMA_SERVER_MODEL=qwen2.5-1.5b-instruct \
    cargo run -p local-voice-cascade --features local --example talk
```

One pipeline declaration under `offline_strict`, validated and planned:

| Node | Role |
|---|---|
| `split` | fan-out: every frame to the detector and the STT node |
| `vad`, `turn policy` | voice activity and turn completion |
| `stt` | speech recognition |
| `input gate` | passes committed spans and turn events on; holds back hypotheses |
| `agent` | the LLM: one generation per turn |
| `output gate` | passes a segment to TTS only with its commit |
| `tts` | speech synthesis |
| `speaker` | plays audio and alone confirms playback, over a feedback edge |

Without the `local` feature, every model node is a testkit mock. With it, `stt` is sherpa-onnx,
`agent` is a loopback `llama-server` and `tts` is still the mock synthesiser until a local TTS
adapter exists <span class="st st-plan">planned</span>. Barge-in supersedes the generation,
discards unplayed segments and stops the speaker.

The live tests are ignored without their variables:

```sh
cargo test -p local-voice-cascade --all-features -- --ignored
```
