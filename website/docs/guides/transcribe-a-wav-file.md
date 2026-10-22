---
title: Transcribe a WAV file
sidebar_position: 4
description: Use one contract and one local adapter, with no runtime, to print the spans sherpa-onnx commits for a WAV file.
lede: The STT contract and the sherpa-onnx adapter alone turn a WAV file into committed spans, offline.
source: examples/stt-sherpa-onnx (examples/transcribe.rs, src/lib.rs, tests/wav.rs, tests/live.rs), adapters/inference-sherpa-onnx/src/stt.rs
---

# Transcribe a WAV file

The `stt-sherpa-onnx` example depends on `inference-stt` and the sherpa-onnx adapter's `stt`
feature only. It reads a WAV file (16-bit PCM, 32-bit float, G.711 mu-law or A-law, any rate and
channel count), streams it in 20 ms frames through the sherpa-onnx recogniser and prints each
span it commits.

:::warning[Requirements]

Linux x86_64, `task sherpa-onnx-archive` run once, and the pinned checkpoint
`sherpa-onnx-streaming-zipformer-en-2023-06-26` unpacked into one directory, as described in
[Run the voice cascade offline](run-the-local-voice-cascade-offline.md#1-get-a-recogniser-checkpoint).
Nothing is downloaded.

:::

## Run it

The checkpoint ships a LibriSpeech clip, `test_wavs/0.wav`:

```sh
INFERENCE_SHERPA_ONNX_STT_DIR=<checkpoint dir> \
    cargo run -q -p stt-sherpa-onnx --example transcribe -- <checkpoint dir>/test_wavs/0.wav
```

```text
committed: AFTER EARLY NIGHTFALL THE YELLOW LAMPS WOULD LIGHT UP HERE AND THERE THE SQUALID QUARTER OF THE BROTHELS
```

The checkpoint is trained on read audiobook speech, and does not recognise synthetic speech
reliably.

Without the variable, the example says what it needs and exits with status 1:

```sh
cargo run -q -p stt-sherpa-onnx --example transcribe -- clip.wav
```

```text
set INFERENCE_SHERPA_ONNX_STT_DIR to the directory of the unpacked pinned checkpoint
```

The WAV decoding is held by
[`examples/stt-sherpa-onnx/tests/wav.rs`](https://github.com/beyond10x/inference/blob/main/examples/stt-sherpa-onnx/tests/wav.rs);
the clip's transcript by the ignored live test in
[`tests/live.rs`](https://github.com/beyond10x/inference/blob/main/examples/stt-sherpa-onnx/tests/live.rs).
