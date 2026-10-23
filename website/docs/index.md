---
slug: /
title: Overview
sidebar_label: Overview
sidebar_position: 1
description: What inference is, what it is not, and where it sits among its neighbours.
lede: inference is a set of Rust libraries for speech and language products whose models you can change without rewriting the product.
source: crates/, adapters/ and examples/ in the inference repository at 0.8.0, CHANGELOG.md
---

# inference

inference is an open-source Rust workspace for **speech and language AI**, under Apache-2.0.
Speech-to-text, language models, text-to-speech and native speech-to-speech models plug in behind
one contract per model kind, and compose into streaming pipelines that are validated and planned
before they run.

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

| Layer | What it is | Read |
|---|---|---|
| Contracts | Typed interfaces for STT, LLM, TTS, tools and native speech. Each works on its own. | [Contracts](concepts/contracts.md) |
| Adapters | Self-hosted models and hosted providers behind those contracts. Each hosted one is a Cargo feature you switch on. | [Adapters](reference/adapters.md) |
| Pipelines | Models, tools and policies as nodes with typed ports, joined by bounded edges, validated and planned before they run. | [Graphs](concepts/graphs.md) |
| Runtime | Runs a planned pipeline with backpressure, cancellation and draining. | [Runtime](concepts/runtime.md) |
| Session | One application-facing interface over any pipeline, cascade or native model. | [Session facade](concepts/session.md) |
| Usage | One usage record per model call, and, with a price document you supply, its cost. | [Usage and cost](concepts/usage.md) |

What has shipped, in which release, and what is planned: [Status](status.mdx).

## What it is not

- Not a model, and not a training framework.
- Not a hosted service: you run it. The repository ships libraries: no binary, server or CLI. The
  runnable programs are examples.
- Not voice-only: a text-only pipeline uses the same parts.
- Not all-or-nothing: one contract and one adapter work without the runtime. The
  [first example](getting-started.md) depends on the STT contract and a mock alone.

## Neighbours

| Project | Relation |
|---|---|
| [llm](https://beyond10x.github.io/llm/) ([source](https://github.com/beyond10x/llm)) | `inference-llm` wraps llm's model contract, `llm_core::Model`; the facade's `cost` feature prices usage with `llm-cost`. |
| [ESS](https://beyond10x.github.io/ess/) ([source](https://github.com/beyond10x/ess)) | The behaviour is specified first in ESS, one domain per layer under `ess/`, and its scenarios run as the conformance suite of `task check`. |
| [Secrets](https://beyond10x.github.io/secrets/) ([source](https://github.com/beyond10x/secrets)) | The OpenAI adapter's `keychain` feature reads the API key through Secrets. |

## Where to go next

- [Getting started](getting-started.md): run the first example, then add inference to a project.
- [Concepts](concepts/overview.md): the vocabulary, from one model call to a voice agent.
- [Guides](guides/talk-to-a-hosted-voice-model.md): talk to a hosted voice model, run the voice
  cascade offline, compare providers, price usage.
- [Limitations](status/limitations.md): what does not exist yet and what each adapter cannot do.
