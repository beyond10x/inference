# Contracts

A contract crate says what a kind of model must do and carries no concrete model, backend or
provider. A concrete integration is an [adapter](../adapters.md) crate that depends on the
contracts, never the reverse. Each contract works alone: a consumer of speech recognition depends
on `inference-stt` and one adapter.

| Crate | Holds |
|---|---|
| `inference-core` | Models, adapters, capabilities, locality, deployment profiles, identifiers, generations; the usage record <span class="st st-rel">0.6.0</span> |
| `inference-audio` | Audio frames, formats, sample-clock positions. No codec or resampler. |
| `inference-text` | Inbound spans and turns; outbound output segments. |
| `inference-stt` | `Recogniser`: audio frames in, transcript hypotheses, revisions and commits out. |
| `inference-llm` | `Generator`: a thin wrapper over the `llm` project's model contract (`llm_core::Model`, tag 0.8.0) that adds generation identity, segment commits and input during output. |
| `inference-tts` | `Synthesiser`: committed segments in, audio frames out; playback is confirmed by the sink. |
| `inference-tools` | `Tool`, `ToolCall` lifecycle, the executor contract and retry policy by side-effect class. |
| `inference-duplex` | `NativeSpeechModel`: one multiport session that takes audio and control and gives audio, transcripts and tool calls. Never an STT, LLM and TTS in disguise. |

## Capabilities are claims, checked

Every adapter carries a record of what it claims. The claims are distinct, and a persistent
connection proves none of them:

| Capability | Means |
|---|---|
| `output_streaming` | emits partial results before completion |
| `incremental_input` | accepts input before the logical input is finished |
| `concurrent_input_output` | accepts more input while emitting output |
| `native_duplex` | the model really computes on simultaneous inbound and outbound streams |
| `revisable_input` | handles provisional source corrections under an agreed policy |
| `tool_calling` | requests structured tools and accepts correlated results |
| `interruptible` | stops work within a declared latency contract |
| `resumable_model_state` | restores equivalent model state after interruption |
| `audio_text_alignment` | reports timing between output audio and text |
| `orchestrated_duplex` | a coordinator restarting ordinary calls; reported apart from `native_duplex`, never as it |

The contracts enforce what they can observe. A recogniser that does not claim
`incremental_input` cannot take frames before end of input; one that does not claim
`output_streaming` cannot emit a span before it. A `Generator` refuses an adapter record that
claims what its model does not declare (`CapabilityNotDeclared`), and input during output only
reaches a model whose adapter claims `concurrent_input_output`. Each start past the adapter's
declared maximum concurrency is refused (`ConcurrencyLimitReached`).

Neither hosted native speech adapter claims `native_duplex`: the duplex crate ships a conformance
probe for that evidence, and neither provider has given it.
