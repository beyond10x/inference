# inference

<p class="lede"><strong>Swap the model.</strong> <span class="accent">Keep the product.</span></p>

inference is an open-source Rust framework for speech and language AI, under Apache-2.0.
Speech-to-text, language models, text-to-speech and native speech-to-speech models plug in behind
one set of contracts and compose into streaming pipelines that are checked before they run.

## The problem

Every voice or language product is wired to the models it launched with.

- The provider's SDK leaks into the application: its event names, its audio formats, its way of
  interrupting, its tool-call shape.
- Changing provider, or running a model yourself, means rewriting the pipeline.
- "Streaming" means different things across providers, and a demo cannot tell you which one you
  bought.
- Interruptions, partial transcripts and tool calls are where conversations break: the model speaks
  over the user, acts on a word that was later corrected, or calls a tool twice.

## What you get

| Layer | What it is | Status |
|---|---|---|
| **Contracts** | Typed interfaces for STT, LLM, TTS, tools and native speech. Each works on its own. | <span class="st st-rel">0.5.0</span> |
| **Adapters** | Self-hosted models and hosted providers behind those contracts. Hosted ones are switched on explicitly. | <span class="st st-rel">0.5.0</span> hosted · <span class="st st-rel">0.6.0</span> local |
| **Pipelines** | Models, tools and policies as nodes with typed ports, joined by bounded edges, validated and planned before they run. | <span class="st st-rel">0.5.0</span> |
| **Runtime** | Runs a pipeline with backpressure, cancellation and draining. | <span class="st st-rel">0.5.0</span> |
| **Session** | One application-facing interface over any pipeline, cascade or native model. | <span class="st st-rel">0.5.0</span> |

## What it is not

- Not a model, and not a training framework.
- Not a hosted service: you run it. The repository ships libraries: no binary, server or CLI.
- Not voice-only: a text-only pipeline uses the same parts.
- Not all-or-nothing: one contract and one adapter work without the runtime
  ([`stt-only`](examples.md#stt-only) depends on the STT contract and a mock alone).

## Status labels

Every page marks what it describes:

| Label | Meaning |
|---|---|
| <span class="st st-rel">0.5.0</span> | In the released tag `0.5.0` (2026-10-20, `CHANGELOG.md`). |
| <span class="st st-rel">0.6.0</span> | In the released tag `0.6.0` (2026-10-20). |
| <span class="st st-rel">0.7.0</span> | In the released tag `0.7.0` (2026-10-21). |
| <span class="st st-plan">planned</span> | In the plan; no code yet. |

Where everything stands: [Status and roadmap](status.md). Back to the [home page](index.html).
