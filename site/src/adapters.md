# Adapters

One crate per provider or backend under `adapters/`, each enabled explicitly, never by default.
Local adapters pin their checkpoint by hash and record its licence, source and revision.

<div class="wide-table">

| Adapter | Contract, runs | Claims | Licences | Environment | Status |
|---|---|---|---|---|---|
| **OpenAI Realtime**<br>`inference-openai` | duplex<br>hosted, WebSocket | output streaming, incremental input, concurrent input/output, tool calling, interruptible | hosted service: audio leaves the machine | `OPENAI_API_KEY` or keychain secret `openai-api-key`<br>`INFERENCE_OPENAI_REALTIME_MODEL` | <span class="st st-rel">0.5.0</span> |
| **Amazon Nova Sonic** on Bedrock<br>`inference-aws` | duplex<br>hosted, bidirectional stream | output streaming, incremental input, concurrent input/output, tool calling, interruptible | hosted service: audio leaves the machine | AWS default credential chain<br>`AWS_REGION`<br>`INFERENCE_NOVA_SONIC_MODEL` | <span class="st st-rel">0.5.0</span> |
| **Microphone and speaker**<br>`inference-audio-device` | audio source and sink<br>in process, cpal | not a model | | | <span class="st st-rel">0.5.0</span> |
| **sherpa-onnx** streaming Zipformer, English<br>`inference-sherpa-onnx`, feature `stt` | STT<br>in process, native FFI | incremental input, output streaming | code Apache-2.0 (sherpa-onnx 1.13.8), MIT (onnxruntime); weights Apache-2.0, trained on LibriSpeech (CC BY 4.0) | `INFERENCE_SHERPA_ONNX_STT_DIR` | <span class="st st-rel">0.6.0</span> |
| **whisper.cpp** Whisper base.en<br>`inference-whispercpp` | STT, batch<br>in process, native FFI, CPU | none: one complete buffer per call | code MIT (whisper.cpp), Unlicense (whisper-rs 0.16.0); weights MIT | `INFERENCE_WHISPERCPP_STT_MODEL`<br>tests: `INFERENCE_WHISPERCPP_STT_CLIP` | <span class="st st-rel">0.6.0</span> |
| **llama-server** Qwen2.5-1.5B-Instruct q4_k_m<br>`inference-llama-server` | LLM<br>local worker, loopback HTTP | output streaming; tool calling with tools on (`--jinja`) | code MIT (llama.cpp); weights Apache-2.0 | `INFERENCE_LLAMA_SERVER_URL`<br>`INFERENCE_LLAMA_SERVER_MODEL` | <span class="st st-rel">0.6.0</span> |
| **Local TTS** | TTS<br>in process | | blocked on a licence decision, see below | | <span class="st st-plan">planned</span> |
| **OpenAI GPT-Live** `gpt-live-1`<br>`inference-openai`, feature `gpt-live` | duplex<br>hosted, WebSocket (`v1/live/sessions`) | native duplex (the probe observed it), output streaming, incremental input, tool calling through Responses delegation | hosted service: audio leaves the machine; billed per session second | keychain secret `openai-api-key` or `OPENAI_API_KEY` | <span class="st st-rel">0.6.0</span> |
| **A self-hosted native speech model** | duplex<br>local | | | | <span class="st st-plan">planned</span> |

</div>

## Notes

- **No `native_duplex` claim** from either hosted adapter: the duplex conformance probe has not
  observed it.
- **Hosted adapters** run at locality `external_provider`, which only a `hybrid_opt_in`
  deployment profile admits ([Offline and privacy](offline-and-privacy.md)).
- **Nova Sonic**: offline tests pass; its live acceptance, one audio exchange with the service, is
  still open in the plan.
- **OpenAI keys** are injected (`CredentialSource`), sent once in the handshake and never logged by
  the adapter. The WebSocket library below it logs the handshake, key included, at trace level:
  keep `tungstenite` targets at debug or lower (for example `RUST_LOG=tungstenite=debug`).
- **sherpa-onnx** links the release archive built without TTS, so no GPL-licensed phonemiser is
  linked. The checkpoint is trained on read audiobook speech and does not recognise synthetic
  speech reliably.
- **whisper.cpp** decodes at most 30 s per window and holds a whole utterance in memory until input
  ends.
- **llama-server**: cancelling closes the connection; the adapter does not claim `interruptible`,
  because whether the server stopped computing is visible only on the server.
- **Local TTS** <span class="st st-plan">planned</span>: sherpa-onnx's TTS build links espeak-ng,
  which is GPL-3.0, against the project's rule that local backend code is Apache-2.0, MIT or more
  permissive. That decision is open.

## Measured on the reference machine

From each adapter's capability record: CPU only, Intel Core i9-10900K, Linux x86_64.

| Adapter | Peak resident memory | Load time | CPU real-time factor | Record |
|---|---|---|---|---|
| sherpa-onnx STT | 363 MiB | 4,937 ms | 0.131 | `adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json` |
| whisper.cpp STT | 337 MiB | 630 ms | 0.202 | `adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json` |
| llama-server LLM | 1,839 MiB | 13,911 ms | n/a | `adapters/inference-llama-server/capability/reference-machine-cpu.json` |

Each record also names the tests that are the evidence for each claim.
