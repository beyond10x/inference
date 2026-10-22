---
title: Local voice cascade
sidebar_position: 2
description: The cascade's nodes, its two commit gates, how a barge-in travels through it, and how the same declaration binds to mocks or to local adapters.
lede: One pipeline declaration under offline_strict - mocks without the local feature, sherpa-onnx or whisper.cpp and a loopback llama-server with it.
source: examples/local-voice-cascade (src/lib.rs, src/local.rs, tests/gates.rs, tests/barge_in.rs), crates/inference-testkit
---

# Local voice cascade

`local-voice-cascade` declares STT, LLM and TTS as typed pipeline nodes with the builder, under the
`offline_strict` profile with egress denied, and validates and plans it before anything runs.

## The nodes

| Node | Role |
|---|---|
| `split` | fan-out: every frame to the detector and the STT node |
| `vad`, `turn policy` | voice activity and turn completion |
| `stt` | speech recognition |
| `input gate` | passes committed spans and turn events on; holds back hypotheses |
| `turn split` | fan-out: the turn events to the LLM and the speaker |
| `agent` | the LLM: one generation per turn |
| `text split` | fan-out: the LLM's output to the output gate and the egress |
| `output gate` | passes a segment to TTS only with its commit |
| `tts` | speech synthesis |
| `speaker` | plays audio and alone confirms playback, over a feedback edge |

The application feeds the audio ingress at the input of `split`, and reads two egresses: the
committed spans of the STT node, and the LLM's output. Every edge and exposure is bounded and
pauses on overload.

## Two bindings of one declaration

| Node | Without feature `local` | With feature `local` |
|---|---|---|
| `stt` | the testkit's mock recogniser | sherpa-onnx or whisper.cpp, chosen by `INFERENCE_CASCADE_STT` ([guide](../guides/swap-the-recogniser-by-configuration.md)) |
| `agent` | the testkit's mock generator | `llama-server` on loopback, a local worker |
| `tts` | the testkit's mock synthesiser | still the mock synthesiser: a local TTS adapter is planned |

The `stt` node requires no capability, so a batch recogniser that claims neither incremental input
nor output streaming plans there as a streaming one does. The cascade's effective capabilities are
what its three adapters all claim.

## Barge-in, step by step

A turn that opens while the answer of an earlier turn is not all played is a barge-in:

1. The turn events reach the speaker and the LLM. At `TurnOpened` the speaker stops, clears its
   queue and reads the played position; from then on it drops every frame of an earlier generation
   and makes no receipt for it.
2. The LLM supersedes its current generation and sends `GenerationSuperseded` on.
3. The output gate drops what it holds of that generation; the TTS node discards each of its
   segments not yet played.

A turn that opens once the answer was all played interrupts nothing. The behaviour is tested on
the mocks and a fake audio host:

```sh
cargo test -q -p local-voice-cascade --test gates --test barge_in
```

```text
running 1 test
.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s


running 1 test
.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
```

To run it against a microphone and the local models, follow
[Run the voice cascade offline](../guides/run-the-local-voice-cascade-offline.md).
