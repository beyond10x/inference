---
format: aep.planning-md/3
id: product-requirements:inference
kind: product-requirements
status: draft
title: 'inference: product requirements'
summary: The eleven goals, nine non-goals and fourteen acceptance rows of the inference design, with the owner decisions of 2026-10-18.
revision: 1
---
## Purpose

`inference` is a Rust composition library for AI inference: contracts for speech-to-text, language
models and text-to-speech as interchangeable typed nodes, composed into validated streaming graphs,
executed by a runtime, with an optional session facade above. Self-hosted and open-source first;
hosted providers are opt-in adapters. Source: `docs/design/inference.md` v0.2 (2026-10-16), §1
(lines 15-54).

## Users

- A Rust application that embeds one STT, LLM or TTS adapter without the graph runtime (§1.1, line 41; A-01).
- An application that composes a pipeline of typed nodes and runs it (§10, §12).
- A voice agent that wants one session interface regardless of topology (§13).
- Research that exports time-aligned traces for controller or duplex-model training (§22).

## Goals

From `docs/design/inference.md:62-72`.

| id | requirement |
|---|---|
| G-01 | Independently usable Rust contracts for STT, LLM, TTS and future modalities |
| G-02 | Models interchangeable through typed adapters and explicit capability negotiation |
| G-03 | Streaming pipelines as graphs of sources, transforms, model nodes, tools, routers and sinks |
| G-04 | One-shot, turn-based, streaming-input, streaming-output and native duplex executions |
| G-05 | Local libraries and self-hosted services without an external cloud dependency |
| G-06 | Optional hosted-provider adapters later, without changing core contracts or pipeline semantics |
| G-07 | Provisional/committed input distinctions and safe output and tool commitment barriers |
| G-08 | Bounded backpressure, cancellation, deterministic control-event replay, resource admission |
| G-09 | Optional unified session interfaces across different pipeline topologies |
| G-10 | Model experimentation, evaluation and time-aligned trace export, without bundling training |
| G-11 | Clear separation between inference work, pipeline orchestration and application/tool permissions |

## Non-goals

From `docs/design/inference.md:76-84`: no neural kernels from scratch; no training of a new model
in the first release; no pretence that STT, LLM and TTS share an architecture; no native duplex
required of decoder-only LLMs; no guarantee of resumable KV caches; no exactly-once external side
effects; no binding of the public contracts to HTTP, WebSockets, WebRTC, Tokio, CUDA, ONNX or one
provider SDK; no implicit conversion that hides incompatible modalities; no mandatory server or
runtime when embedding one adapter.

## Acceptance

From `docs/design/inference.md:1137-1150`. Each row is claimed by at least one epic; the epic's
exit names the conformance scenarios and tests that demonstrate it.

| id | required result | epic |
|---|---|---|
| A-01 | depend on only `inference-stt` and one adapter, no runtime, no TTS | contracts-and-testkit, local-stt-adapter |
| A-02 | build and execute a text-only streaming pipeline | graph-minimum, local-llm-adapter |
| A-03 | the same graph API describes an offline STT → LLM → TTS cascade | local-voice-cascade |
| A-04 | the local cascade executes with external network access denied | local-voice-cascade |
| A-05 | a different STT, TTS or LLM adapter replaces its predecessor without changing contracts | backend-replacement |
| A-06 | a native speech model is one multiport node, not three fake components | native-voice |
| A-07 | true duplex and orchestrated incremental output are different effective capabilities | duplex-research |
| A-08 | STT revisions and speculative LLM output cannot cross irreversible boundaries by accident | contracts-and-testkit, local-voice-cascade |
| A-09 | tool proposals are schema-validated, authorized, correlated and auditable | tools-and-turns |
| A-10 | interruption stops unplayed speech and suppresses stale generations | local-voice-cascade |
| A-11 | a hosted adapter can be enabled explicitly while the offline profile refuses it | hosted-and-scale-out |
| A-12 | a session switches among supported topologies without changing its event vocabulary | session-facade |
| A-13 | graph construction rejects a capability mismatch with an actionable error | graph-minimum |
| A-14 | reusable subgraphs compose without losing cancellation and tracing boundaries | session-facade |

## Owner decisions (2026-10-18)

- The library ships no binary, server or CLI. The core and the domain contract crates carry no
  concrete model, backend or provider implementation; every integration is an adapter crate below
  them.
- `llm`, `llm-gateway` and Loom are treated as absent. An adapter to one may be written later;
  `inference` may replace `llm`. Neither is planned here.
- Spec first: every noun is declared in `ess/` before a story is written around it, and a story's
  acceptance is a set of named conformance scenarios.
