---
title: Contracts
sidebar_position: 2
description: What a model kind must do, with no model inside, and the capability claims an adapter makes and the contracts check.
lede: A contract crate carries no concrete model; an adapter implements it and claims capabilities the contract enforces where it can observe them.
source: crates/inference-core (model.rs), crates/inference-stt, crates/inference-llm, crates/inference-duplex (conformance.rs), adapters/inference-openai/src/live_duplex.rs
---

# Contracts

A contract crate says what a kind of model must do and carries no concrete model, backend or
provider. A concrete integration is an [adapter](../reference/adapters.md) crate that depends on
the contracts, never the reverse. Each contract works alone: a consumer of speech recognition
depends on `inference-stt` and one adapter.

| Model kind | Contract | Crate |
|---|---|---|
| speech-to-text | `Recogniser`: audio frames in; transcript hypotheses, revisions and commits out | `inference-stt` |
| language model | `Generator`: a wrapper over [llm](https://beyond10x.github.io/llm/)'s `llm_core::Model` that adds generation identity, segment commits and input during output | `inference-llm` |
| text-to-speech | `Synthesiser`: committed segments in, audio frames out; playback is confirmed by the sink | `inference-tts` |
| tools | `Tool`, the `ToolCall` lifecycle, the executor contract and retry by side-effect class | `inference-tools` |
| native speech | `NativeSpeechModel`: one multiport session that takes audio and control and gives audio, transcripts and tool calls; never an STT, LLM and TTS in disguise | `inference-duplex` |

The shared vocabulary (models, adapters, capabilities, locality, deployment profiles, identifiers,
the usage record) is in `inference-core`; audio frames and formats in `inference-audio`; spans,
turns and output segments in `inference-text`. Every crate is listed in
[Crates](../reference/crates.md).

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

## `native_duplex` takes evidence

`inference-duplex` ships a conformance procedure, `native_duplex_evidence`, that sends input while
the model is speaking and looks for output on the same response that took that input up. Of the
three hosted native speech adapters, only GPT-Live claims `native_duplex`: the procedure, run
against hosted `gpt-live-1`, recorded that evidence. OpenAI Realtime and Nova Sonic do not claim
it. Which adapter claims what, and which test holds each claim, is in the claims matrix of
[Adapters](../reference/adapters.md).
