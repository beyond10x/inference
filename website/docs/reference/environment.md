---
title: Environment
sidebar_position: 4
description: Every environment variable the adapters, examples and live tests read, what it does, and its default.
lede: Adapters and examples take their keys, checkpoints and servers from the environment or from options; nothing is looked up or downloaded on their behalf.
source: adapters/inference-openai (config.rs, credentials), adapters/inference-aws (nova_sonic), adapters/inference-elevenlabs (credential.rs, config.rs, examples/live.rs, tests/agent_duplex_live.rs, tests/stt_live.rs, tests/tts_cancel_live.rs), adapters/inference-deepgram (credential.rs, tests/live.rs), adapters/inference-phonic, adapters/inference-deepslate and adapters/inference-cartesia (credential.rs, config.rs, tests/live.rs), examples/realtime-voice, examples/local-voice-cascade (src/local.rs), examples/stt-sherpa-onnx, adapter tests
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
| `PHONIC_API_KEY` | `inference-phonic` (`EnvironmentSource::phonic`), its live test; `realtime-voice`, example `compare` | the Phonic key, sent only in the handshake's `Authorization` header | none: the `phonic` arm of `compare` fails; the live test also reads the OS keychain, secret `phonic-api-key` |
| `DEEPSLATE_API_KEY` | `inference-deepslate` (`EnvironmentSource::deepslate`), its live test; `realtime-voice`, example `compare` | the Deepslate key, sent only in the handshake's `Authorization` header | none: the session fails to open; the live test also reads the OS keychain, secret `deepslate-api-key` |
| `DEEPSLATE_VENDOR_ID`, `DEEPSLATE_ORGANIZATION_ID` | `inference-deepslate` (`Endpoint::from_env`) | the vendor and organization the Opal endpoint is opened for | none: `ConfigError::Missing` |
| `DEEPSLATE_MODEL` | `inference-deepslate` (`Endpoint::from_env`) | another Opal model | the adapter's `MODEL` |
| `DEEPSLATE_VOICE_ID` | `inference-deepslate` live test; `realtime-voice`, example `compare` | the hosted voice Opal speaks with | none: the `deepslate` arm of `compare` fails |
| `CARTESIA_API_KEY` | `inference-cartesia` (`EnvironmentSource::cartesia`); `realtime-voice`, example `compare` with `--cascade-voice cartesia` | the Cartesia key, sent only in the `X-API-Key` header | none: the cascade arm of `compare` is skipped; the live test also reads the OS keychain, secret `cartesia-api-key` |
| `CARTESIA_VOICE_ID` | `inference-cartesia` live test; `realtime-voice`, example `compare` with `--cascade-voice cartesia` | the Sonic voice the cascade arm speaks with | none: the cascade arm is skipped |
| `INFERENCE_COMPARE_BARGE_IN_WAV` | `realtime-voice`, example `compare` | a 16-bit PCM WAV file of speech sent over each answer as a barge-in | no barge-in |
| `ELEVENLABS_API_KEY` | `inference-elevenlabs` (`EnvironmentSource::elevenlabs`), its example `live` and test `stt_live`; `realtime-voice`, examples `compare` and `talk` (`--provider elevenlabs-cascade`) | the ElevenLabs key, sent only in the `xi-api-key` header, read at every connection | the example `live`, the test `stt_live`, `talk` and the cascade arm of `compare`, for its synthesiser and, with `--cascade-stt elevenlabs`, its recogniser, read the OS keychain, secret `elevenlabs-api-key`; `talk` exits with status 2 without either, or when the keychain gives no answer within 8 s, and the cascade arm of `compare` is skipped |
| `ELEVENLABS_RESIDENCY` | `inference-elevenlabs` (`Endpoint::from_env`), its example `live` and tests `stt_live` and `tts_cancel_live`; `realtime-voice` (`elevenlabs::endpoint`), the cascade arm of `compare` and `talk --provider elevenlabs-cascade` | the data-residency host the key belongs to: `global`, `us`, `eu`, `in` or `sg`; every other host refuses a key of a residency stack, and any other value is refused (`ConfigError::Residency`, which does not print it): `talk` exits with status 2, the cascade arm of `compare` is skipped | empty or unset: `global`, `wss://api.elevenlabs.io` |
| `INFERENCE_ELEVENLABS_STT_CLIP` | `inference-elevenlabs` test `stt_live` (features `live` and `stt`) | a German speech clip of about 30 s in G.711 μ-law at 8 kHz, mono, as a WAV file of format 7 or bare μ-law bytes, which the live check transcribes | none: the test panics |
| `ELEVENLABS_VOICE_ID` | `inference-elevenlabs`, example `live`; `realtime-voice`, examples `compare` and `talk` (`--provider elevenlabs-cascade`) | the voice the live check, the cascade arm of `compare` and the ElevenLabs cascade of `talk` speak with; with the ElevenLabs key it configures the cascade arm's hosted TTS | none: the example `live` and `talk` exit with status 2 and the cascade arm is skipped |
| `ELEVENLABS_AGENT_ID` | `inference-elevenlabs` test `agent_duplex_live`, `realtime-voice` example `compare` | the ElevenLabs agent the live check and `compare` talk to | none: one of it and `ELEVENLABS_AGENT_SIGNED_URL` is required |
| `ELEVENLABS_AGENT_SIGNED_URL` | as above | a signed conversation URL, used instead of the agent id and the key | none |
| `DEEPGRAM_API_KEY` | `inference-deepgram` (`EnvironmentSource::deepgram`), its live tests | the Deepgram key, sent only as `Authorization: Token` in the handshake | none; with feature `keychain`, `KeychainSource` reads the OS keychain |
| `INFERENCE_DEEPGRAM_STT_CLIP` | `inference-deepgram` test `live` | a 16-bit mono WAV file the live check transcribes | none: the test panics |

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

The `cascade` arm of the `realtime-voice` example `compare`, built with feature `local`, reads the
same five variables as `talk`: `INFERENCE_CASCADE_STT`, `INFERENCE_SHERPA_ONNX_STT_DIR`,
`INFERENCE_WHISPERCPP_STT_MODEL`, `INFERENCE_LLAMA_SERVER_URL` and `INFERENCE_LLAMA_SERVER_MODEL`.
It has no options for them. A missing one skips the arm, naming the missing step. With
`--cascade-stt elevenlabs` the arm reads only `INFERENCE_LLAMA_SERVER_URL` and
`INFERENCE_LLAMA_SERVER_MODEL` of these: its recogniser is ElevenLabs, with the key
`ELEVENLABS_API_KEY` or the OS keychain's `elevenlabs-api-key`. A missing one, or a URL whose host
is not loopback, skips the arm, naming it. Any server with an OpenAI-compatible Chat Completions
API on loopback is taken: Ollama's is at `http://127.0.0.1:11434/v1`.

The `realtime-voice` example `talk` with `--provider elevenlabs-cascade`, built with feature
`local`, reads only `INFERENCE_LLAMA_SERVER_URL` and `INFERENCE_LLAMA_SERVER_MODEL` of these: its
recogniser is ElevenLabs. A missing one, or a URL whose host is not loopback, exits with status 2,
naming it.

`talk` also takes `--max-output-tokens` (the most tokens of one answer, 64 by default), and `--input`
and `--output` to pick devices by name.

## Build

| Variable | Set by | Effect |
|---|---|---|
| `SHERPA_ONNX_LIB_DIR` | `.cargo/config.toml` | where the build finds the sherpa-onnx native library that `task sherpa-onnx-archive` unpacks under `target/sherpa-onnx` |
