# `inference` — Rust Inference Contracts, Adapters, Pipelines & Runtime

**Document type:** System architecture and design specification  
**Status:** Proposed / design draft  
**Version:** 0.2  
**Date:** 2026-10-16  
**Implementation language:** Rust  
**Repository:** `inference` (single Cargo workspace, multiple crates)  
**Operating principle:** Open-source and self-hosted first; external providers optional

> **Central design:** Define the *contracts* for AI inference separately from model implementations. Treat each model or transformation as a node with typed input/output ports. Compose nodes into validated, streaming graphs. Run those graphs in a Rust runtime, and optionally expose a uniform application-facing session facade. No application should have to adopt the full system to use one STT, LLM, or TTS adapter.

---

## 1. Executive summary

`inference` is a modular Rust framework for building applications and pipelines from interchangeable AI inference components. It is **not** itself a foundation model, a training framework, a mandatory web server, or a voice-agent-only product.

The repository contains separate crates for:

- **Core contracts:** common types, capabilities, errors, event envelopes, and streaming semantics.
- **Inference domains:** speech-to-text (STT), language models (LLMs), text-to-speech (TTS), and optional multimodal/duplex capabilities.
- **Adapters:** concrete integrations with self-hosted models, inference servers, native libraries, or optional hosted APIs.
- **Pipelines:** typed graph declarations, connections, compatibility checking, and composition.
- **Runtime:** execution, resource admission, buffering, cancellation, scheduling, recovery, and observability.
- **Facades:** optional higher-level sessions for applications that want one input/output interface regardless of pipeline topology.

The architecture must support, among other applications:

1. **Text inference:** prompt → text LLM → streamed text.
2. **Cascaded voice:** audio → STT → LLM ↔ tools → TTS → audio.
3. **Speech-aware hybrid:** audio → audio-capable model → TTS.
4. **Native voice:** audio ↔ speech-to-speech model ↔ audio.
5. **Experimental duplex text:** streaming audio → STT → simultaneously reading/writing text model or policy → TTS.
6. **Other graphs:** transcription, speech translation, RAG, classification, embeddings, batch processing, and event-driven agent workflows.

The **recommended first vertical slice** is a completely local cascaded voice pipeline backed by a modest set of adapters and a text-only pipeline for testing. Native speech-to-speech and duplex-text research should follow after streaming and graph semantics are well defined.

### 1.1 Why this is one repository

A single Cargo workspace gives the project one design vocabulary and shared conformance tests while preserving independently consumable crates. A consumer interested only in STT should not pull in TTS, an LLM server, a graph runtime, or an HTTP transport.

### 1.2 Fundamental invariant

**A persistent connection is not proof of a streaming model.** These are distinct capabilities:

- Streaming output after receiving a complete input.
- Accepting incremental input before its logical completion.
- Accepting new input *while* generating output.
- Genuine native concurrent (duplex) processing inside a model/serving protocol.

The framework MUST describe, negotiate, and test these differences accurately. A coordinator that repeatedly restarts ordinary LLM calls can provide an *orchestrated duplex experience*; it MUST NOT claim that the underlying LLM is natively duplex.

---

## 2. Scope, goals, and non-goals

### 2.1 Goals

| ID | Requirement |
|---|---|
| G-01 | Provide independently usable Rust contracts for STT, LLM, TTS, and future inference modalities. |
| G-02 | Make models interchangeable through typed adapters and explicit capability negotiation. |
| G-03 | Build streaming pipelines as graphs of sources, transforms, model nodes, tools, routers, and sinks. |
| G-04 | Support one-shot, turn-based, streaming-input, streaming-output, and genuinely duplex executions. |
| G-05 | Support local libraries and self-hosted services without an external cloud dependency. |
| G-06 | Permit optional hosted-provider adapters later without changing core contracts or pipeline semantics. |
| G-07 | Preserve provisional/committed input distinctions and safe output/tool commitment barriers. |
| G-08 | Provide bounded backpressure, cancellation, deterministic control-event replay, and resource admission. |
| G-09 | Give applications optional unified session interfaces across otherwise different pipeline topologies. |
| G-10 | Permit model experimentation, evaluation, and time-aligned training-trace export without bundling training into the runtime. |
| G-11 | Maintain a clear separation between inference work, pipeline orchestration, and application/tool permissions. |

### 2.2 Explicit non-goals

- Implementing neural inference kernels from scratch merely because Rust is the host language.
- Training a new general-purpose LLM, speech model, or speech codec in the first release.
- Pretending all STT/LLM/TTS models share an internal neural architecture.
- Requiring native duplex support from traditional decoder-only language models.
- Guaranteeing all adapters can resume model KV caches or proprietary session state.
- Guaranteeing exactly-once external side effects from retries or network delivery.
- Binding the public domain contracts to HTTP, WebSockets, WebRTC, Tokio, CUDA, ONNX, or any one provider SDK.
- Providing an all-powerful implicit conversion mechanism that hides incompatible modalities or capabilities.
- Mandating a server or a pipeline runtime when embedding a single model adapter in a Rust application.

### 2.3 Normative language

In this specification **MUST** indicates a hard contract, **SHOULD** indicates a default recommendation with documented exceptions, and **MAY** indicates an optional extension.

---

## 3. Conceptual model and vocabulary

The project must not confuse **models**, **adapters**, **nodes**, **pipelines**, **runtimes**, and **sessions**.

| Concept | Definition | Owns |
|---|---|---|
| **Model** | A trained inference artifact or hosted model endpoint, described by capabilities and resources. | Model identity, modality support, constraints. |
| **Adapter** | A concrete implementation connecting a model/backend to framework contracts. | Model-specific transport, decoding, backend lifecycle. |
| **Node** | An executable processor with named typed input and output ports. Can wrap a model or be deterministic. | Port behavior, node-local execution. |
| **Port** | An input or output endpoint carrying a declared family of typed events/media. | Shape and compatibility requirements. |
| **Edge** | A bounded channel connecting an output port to a compatible input port. | Routing and delivery policy. |
| **Pipeline** | A validated graph of nodes and edges with declared inputs and outputs. | Composition and requested behavior. |
| **Graph execution** | One invocation or long-lived run of a pipeline. | Runtime scheduling, health, cancellation. |
| **Session** | An optional application-level interaction lifetime, potentially spanning multiple graph runs and model requests. | History, logical identity, interaction policy. |
| **Tool** | An external capability invoked through a controlled request/result protocol. | Permission-gated side effects or data retrieval. |
| **Facade** | Higher-level API hiding topology details while exposing negotiated behavior. | Application interaction contract. |

### 3.1 Model modalities are not neural architecture categories

STT and TTS are inference tasks, not necessarily LLMs. STT can use conformers, transducers, encoders, or transformers; TTS can use codec-language models, flow models, diffusion, or other speech synthesizers. Native speech models may combine several tasks in one network.

The framework MUST classify them by **externally observable contract**, not by internal neural design. It MUST also permit one model to expose several capabilities without inventing separate underlying model instances.

### 3.2 Three primary abstractions

1. **Model / adapter:** what computation can be performed, and what it requires.
2. **Node:** how that computation participates in a typed event graph.
3. **Pipeline:** how independently usable nodes are wired and coordinated.

The **runtime** executes the pipeline; the **facade** is an optional application convenience. These remain distinct so that a small library user is not forced into a session server architecture.

---

## 4. High-level system architecture

```text
                            Application(s)
        transcription  |  voice agent  |  RAG  |  research  |  batch
                                  |
                    +-------------+-------------+
                    |                           |
            Direct crate APIs           Optional session facade
                    |                           |
                    |                    inference-facade
                    |                           |
                    +-------------+-------------+
                                  |
                       inference-pipeline
                    Typed graph declarations
                     Ports / edges / policies
                                  |
                       inference-runtime
           Execute / schedule / cancel / route / observe
                                  |
           +----------------------+---------------------+
           |                      |                     |
       Local adapters         Tool nodes           Hosted adapters
      (default choice)      / MCP bridges         (explicit opt-in)
           |                      |                     |
      Whisper / Kyutai      Local functions        OpenAI / Anthropic /
      llama.cpp / Moshi     and services           Gemini / others
           |                      |                     |
           +----------------------+---------------------+
                                  |
                  inference-core and domain contracts
                    (text / audio / STT / LLM / TTS)
```

This diagram shows logical interaction, **not Cargo dependency direction**. In Cargo, contracts sit at the bottom; concrete adapters and the runtime depend on contracts. The runtime consumes node implementations via interfaces rather than depending on any specific backend.

### 4.1 Planes and boundaries

- **Data plane:** text fragments, transcripts, media frames, token/output deltas, tool data, and transformations along graph edges.
- **Control plane:** graph activation, capability negotiation, authorization, generation control, session management, budgets, and cancellation.
- **Execution plane:** model processes, GPU resources, local services, tool workers, and remote network clients.
- **Observation plane:** structured traces, metrics, replay, diagnostics, and audit events.

Data transport and control transport MAY share an underlying channel. Their semantics MUST remain distinguishable.

---

## 5. Repository and Cargo workspace design

**Repository name:** `inference`  
**Packaging:** single version-controlled repository; multiple versioned Cargo crates.

```text
inference/
├── Cargo.toml                      # Workspace members and shared metadata
├── README.md
├── LICENSES/                       # Repository + third-party notices
├── docs/
│   ├── architecture.md
│   ├── contracts.md
│   ├── streaming-semantics.md
│   ├── pipeline-spec.md
│   ├── adapter-conformance.md
│   └── adr/
├── crates/
│   ├── inference-core/             # Events, IDs, errors, features, lifecycle
│   ├── inference-text/             # Text spans, revisions, segment contracts
│   ├── inference-audio/            # Audio frames, formats, timing contracts
│   ├── inference-stt/              # Speech recognition interfaces
│   ├── inference-llm/              # Language generation and tool-call interfaces
│   ├── inference-tts/              # Speech synthesis interfaces
│   ├── inference-duplex/           # Optional simultaneous I/O interfaces
│   ├── inference-tools/            # Tool definitions, calls, permissions
│   ├── inference-pipeline/         # Graph declarations and validation
│   ├── inference-runtime/          # Graph execution and scheduling
│   ├── inference-facade/           # Optional logical session API
│   ├── inference-testkit/          # Mock nodes, traces, conformance fixtures
│   └── inference/                  # Optional convenience re-export crate
├── adapters/
│   ├── inference-llamacpp/         # Local text model serving
│   ├── inference-vllm/             # Local/server text model serving
│   ├── inference-whispercpp/       # Local speech recognition
│   ├── inference-sherpa/           # Local speech inference utility integration
│   ├── inference-kyutai/           # Streaming STT/TTS backend integration
│   ├── inference-moshi/            # Native duplex speech integration
│   ├── inference-openai/           # Optional hosted model/voice integration
│   ├── inference-anthropic/        # Optional hosted text inference
│   └── inference-google/           # Optional hosted text/audio integration
├── examples/
│   ├── stt-only/
│   ├── llm-only/
│   ├── tts-only/
│   ├── text-tools/
│   ├── local-voice-cascade/
│   ├── native-voice/
│   └── voice-translation/
└── tests/
    ├── contracts/
    ├── adapters/
    ├── pipelines/
    └── integration/
```

### 5.1 Responsibility and dependency table

| Crate | Public responsibility | Allowed dependencies |
|---|---|---|
| `inference-core` | Minimal shared primitives and semantics | Small stable utility dependencies only. |
| `inference-text` | Text deltas, spans, revisions, commitments | `core`. |
| `inference-audio` | Audio media descriptors, frames, clocks, formats | `core`. |
| `inference-stt` | Speech recognition domain contract | `core`, `audio`, `text`. |
| `inference-llm` | Text generation, context, structured calls | `core`, `text`, `llm` (`b10x-llm-core`); optional `tools`. |
| `inference-tts` | Speech synthesis domain contract | `core`, `text`, `audio`. |
| `inference-duplex` | Explicit simultaneous input/output semantics | `core`, relevant media contracts. |
| `inference-tools` | Tool schemas, execution contracts, outcomes | `core`, optionally `text`. |
| `inference-pipeline` | Nodes, typed graph descriptions, static validation | `core` and declared port types. |
| `inference-runtime` | Graph execution and operational policies | `pipeline`, `core`, runtime utilities. |
| `inference-facade` | Session-centric public API | `runtime`, domain contracts. |
| `inference-testkit` | Mocks, protocol validation, replay fixtures | Contracts and test-only utilities. |
| `inference` | Optional ergonomic re-exports | Feature-selected crates only. |
| `inference-<backend>` | Concrete implementation(s) of domain/node contracts | Relevant contracts; backend-specific libraries. |

**Dependency rule:** provider adapters MUST NOT be dependencies of `inference-core`, the domain contract crates, or the graph specification crate. Applications select concrete adapters at composition/deployment time.

### 5.2 Dependency direction

```text
inference-core
   |-- inference-text -----------+
   |-- inference-audio ----------+--> inference-stt / inference-llm / inference-tts
   |-- inference-tools ----------+              |
   |-- inference-duplex -----------------------+  |
   |-- inference-pipeline (node + graph spec) -+--+--- concrete adapters
   |                                             |
   +---------------------------------------- inference-runtime
                                                 |
                                          inference-facade
                                                 |
                                       applications and services
```

Actual package dependencies can be leaner than the drawing. The core principle is **contracts are below implementations**, and pipelines depend on *interfaces*, not backend crates.

### 5.3 Keep the initial workspace small

The tree above is a **target structure**, not a request to create empty crates on day one.

A useful first implementation can begin with `inference-core`, `inference-text`, `inference-audio`, `inference-stt`, `inference-llm`, `inference-tts`, `inference-pipeline`, `inference-runtime`, and `inference-testkit`, plus two or three adapters. Split out `duplex`, `tools`, and `facade` only as their stable public contracts become clear.

### 5.4 Versioning

- Crates may be published and versioned independently, but compatible contract revisions SHOULD align through a workspace-wide compatibility policy.
- Public semantic event schemas MUST carry a version when crossing process or network boundaries.
- Adapter descriptors MUST report implemented contract versions.
- Internal Rust APIs need not provide a permanent cross-compiler ABI; stable plugin interoperability is handled separately.

---

## 6. Canonical inference data model

### 6.1 Modalities and payloads

First-class media modalities SHOULD include:

| Modality | Examples | Important semantic details |
|---|---|---|
| Text | UTF-8 segments, token deltas, structured output | Hypothesis vs committed text, tokenization independence. |
| Audio | PCM frames, compressed frames, codec token streams | Sample clock, encoding, sample rate, channels, timestamps. |
| Structured data | Tool arguments, JSON results, classifications | Schema and trust boundary. |
| Image | Pixel buffers, encoded images, image references | Size, format, provenance. |
| Video | Timestamped frame/event streams | Timebase, synchronization, ordering. |
| Control | Start, stop, commit, revise, cancel, error | Must not be mistaken for user content. |

The initial runtime MUST implement text, audio, structured events, and control. Image/video MAY follow without redesigning the top-level event/port model.

### 6.2 Canonical event envelope

Every routable event SHOULD include:

| Field | Meaning |
|---|---|
| `schema_version` | Version of serialized contract, when applicable. |
| `event_id` | Stable identity for deduplication and tracing. |
| `run_id` | The particular graph execution. |
| `session_id` | Optional application-level session. |
| `stream_id` | Independently ordered stream identity. |
| `sequence` | Increasing per-producer/per-stream sequence. |
| `producer` / `port` | Source node and source port. |
| `kind` | Typed event discriminator. |
| `generation_id` | Relevant text/audio/model generation, if any. |
| `timestamp` | Source-relative or normalized monotonic event/media timestamp. |
| `causation_id` | Triggering event or operation, if known. |
| `payload` | Typed value or reference to associated binary data. |

The runtime MUST NOT assume a single global total order across independently produced audio, text, tool, and control streams. A recorder MAY assign an additional merge order to aid replay.

### 6.3 Media ownership and transport

- Audio frames SHOULD travel as bounded binary buffers or references on realtime paths, not base64-encoded JSON.
- Ownership and lifetime of borrowed/zero-copy buffers MUST be explicit for in-process nodes.
- Across IPC/network boundaries, adapters MUST negotiate framing, encoding, size, and ownership semantics.
- A common payload vocabulary is desirable; a universal model tokenizer is **not** required.
- Provider-specific data MAY be retained in namespaced extensions but MUST NOT replace mandatory common fields.

### 6.4 Specialized domain event families

| Family | Representative semantic events |
|---|---|
| STT | `SpeechStarted`, `TranscriptHypothesis`, `TranscriptRevision`, `TranscriptCommitted`, `SpeechEnded`. |
| LLM | `GenerationStarted`, `TextDelta`, `TextSegmentCommitted`, `StructuredOutput`, `GenerationCompleted`. |
| TTS | `SynthesisStarted`, `AudioChunk`, `SynthesisCompleted`, optional text/audio alignment updates. |
| Duplex | `InputAccepted`, `Read`, `Wait`, `Write`, `InputRevised`, `InterruptionObserved`. |
| Tools | `ToolCallProposed`, `ToolCallAuthorized`, `ToolCallStarted`, `ToolCallResult`, `ToolCallFailed`. |
| Session | `SessionOpened`, `SessionReady`, `SessionDraining`, `SessionClosed`. |
| Runtime | `Backpressure`, `DeadlineExceeded`, `NodeFailed`, `RunCancelled`, `CapabilitiesChanged`. |

These are **semantic event names**, not a frozen Rust enum or serialized wire schema. Several events may be variants of shared tagged families in an eventual implementation.

---

## 7. Contracts for STT, LLM, TTS, and duplex systems

All domain crates MUST define observable behavior independently of the model architecture, inference engine, and transport.

### 7.1 STT contract — `inference-stt`

**Input:** timestamped audio or audio stream.  
**Output:** text hypotheses, revisions, committed text segments, optional language/speaker/timing metadata.

Required contract concepts:

- Supported language(s), audio formats, sample rates, and channel limits.
- Batch transcription, incremental audio ingestion, and output streaming as separately declared capabilities.
- Distinction between provisional and immutable committed transcript spans.
- Speech activity and endpoint metadata MAY be exposed but are not mandatory for every STT model.
- Timestamp alignment and speaker diarization are optional, explicitly reported features.
- A chunked offline recognizer may be wrapped in a streaming *pipeline node*, but MUST NOT claim native incremental ASR if it processes complete chunks in independent calls.

### 7.2 LLM contract — `inference-llm`

**Contract:** `llm_core::Model` of the beyond10x `llm` repository (`b10x-llm-core`, at a release tag). One asynchronous `Model::turn` takes a `TurnRequest` (instructions, conversation items, tool specifications, sampling and controls), streams `StreamEvent`s into a sink, and returns a `TurnOutcome` (stop reason, output items and tool calls, usage observation) or a typed `Error`; a `Cancel` token stops a running turn. Request, items, tool calls and usage are `llm`'s types. `llm`'s wires (Responses, Messages, Chat Completions) are provider implementations and belong to adapters, not to the contract crate.

**What `inference-llm` adds:** only what a graph node needs, and no second text-model vocabulary.

- A generation: its identity (run, producing node, epoch) and the events of §6.4 (`GenerationStarted`, `TextDelta`, `TextSegmentCommitted`, `GenerationCompleted`), with supersede, cancel and complete as control that reaches the running turn. A turn that fails cancels the generation and delivers `GenerationCancelled` with the segments it discarded, `llm`'s error kept. The wrapper makes one attempt per turn, as `llm`'s `Model::turn` runs one; a caller that wants a failure before output retried wraps its model in `llm`'s routing, its fallback run, before handing it to the wrapper. A generation is a conversation of turns and no turn asks an empty request: the first on the request it was started with when that carries items, otherwise on the first input; each later one on the conversation so far, once input or the results of a whole tool round arrive, so every turn is a request `llm` accepts. While calls of a round are unanswered, the generation waits for them and does not complete by itself.
- Commit points: each delta is one segment, committed as it is emitted; a turn that showed no delta gives the assistant text of its outcome as one segment when it ends. This rests on `llm`'s rule (`llm` design, "Streamed events are final"): every streamed event is final, each assistant message of a successful turn carries exactly the text deltas shown for it, and a message no delta was shown for carries the text the server sent.
- Capabilities: output streaming, incremental input and input during output MUST be distinct capabilities of the adapter record (§8.1). An adapter record claiming what its `llm` model's `Capabilities` do not declare is refused: `output_streaming` needs `output_streaming`, `tool_calling` needs `tools`, `native_duplex` needs `input_during_output`. `concurrent_input_output` needs no declaration: the wrapper itself accepts input while the generation emits output, as the next turn's user text, since no `llm` protocol carries input into a running turn. Input during output is admitted only where it is claimed; an adapter that does not stream delivers each turn's text as one segment, at completion. The maximum concurrency the adapter record declares is held.
- Tool calls are `llm`'s, taken from a turn's outcome as structured proposals, never as user-visible prose. Behind the `tools` feature, declared tools map onto `llm` tool specifications and the outcomes of the tools domain onto `llm` tool results. A call rejected before it ran is sent as `llm` sends one, a failed result whose output is the caller's own text of the reason (`llm` design, "Failed and rejected tool results"); the tools domain keeps `ToolCallFailed` and `ToolCallRejected` apart. The framework MUST preserve generation IDs and cancellation status across tool round trips.

Context semantics (stateless request, history carried in the items, opaque state bound to the binding that produced it), context limits and token accounting are `llm`'s, and MUST NOT be universally assumed identical. Where `llm` lacks something a graph node needs, it is an `llm` gap, reported there and not re-created here.

### 7.3 TTS contract — `inference-tts`

**Input:** committed or explicitly speculative text segments, synthesis options, voice identity.  
**Output:** audio frames or chunks, timing/alignment and lifecycle events where supported.

Required contract concepts:

- Declare whether input can arrive incrementally or only as a complete utterance.
- Declare whether audio is emitted incrementally or only on synthesis completion.
- Model-specific voices, languages, codecs, audio formats, and speaker conditions are capabilities, not universal assumptions.
- Distinguish *synthesized*, *queued*, and *played* audio. Only the client/media sink can confirm playback.
- Text-to-audio alignment is optional; applications MAY require it at graph validation.
- TTS MUST NOT treat an uncommitted LLM token stream as safe to speak by default.

### 7.4 Duplex contract — `inference-duplex`

**Input and output:** one or more concurrently active typed streams, potentially text, audio, and tools.

Required contract concepts:

- Explicit concurrency policy for accepting inputs while outputs are being produced.
- Rules for interruption, read/write scheduling, and incoming revision handling.
- Native model duplex versus orchestrator-created duplex experience reported separately.
- Whether the adapter accepts new user input mid-generation without restarting inference.
- Whether tool results can be integrated concurrently or only at a safe boundary.
- Multi-output models MAY expose synchronized speech and text ports from the same instance.

### 7.5 No forced decomposition

A native audio-to-audio model MUST be expressible as a **single multiport node**. It need not masquerade as a separate STT node, LLM node, and TTS node. Applications may attach optional transcript/tool sidecars when necessary, but those sidecars are real graph nodes with their own latency, accuracy, and resource costs.

---

## 8. Capability descriptors and negotiation

The ability to connect two ports is insufficient: the runtime must establish that the selected implementations satisfy the *behavioral* requirements of the graph and application.

### 8.1 Core streaming capability matrix

| Capability | Meaning | Common provider behavior |
|---|---|---|
| `output_streaming` | Emits partial results before completion. | Many LLM/TTS/STT servers. |
| `incremental_input` | Accepts input pieces before the logical input is finished. | Streaming STT; some TTS and speech models. |
| `concurrent_input_output` | Accepts additional input while emitting output. | Some native realtime/duplex systems. |
| `native_duplex` | The model and serving stack genuinely compute on simultaneous inbound/outbound streams. | Specialized speech/duplex models. |
| `revisable_input` | Can handle provisional source corrections under an agreed policy. | Specialized duplex systems or orchestrators. |
| `tool_calling` | Can request structured tools and accept correlated results. | Many text LLMs; select realtime models. |
| `interruptible` | Can stop/deactivate work within a declared latency contract. | Depends on serving backend. |
| `resumable_model_state` | Restores equivalent opaque model state after interruption. | Not a general guarantee. |
| `audio_text_alignment` | Reports temporal relationships between output audio and text. | TTS/model dependent. |

### 8.2 Resource and policy descriptors

Additional properties include:

- Model family, checkpoint/version, backend runtime, supported contract revision.
- Accepted/output modalities and media formats.
- Maximum concurrency, expected startup footprint, CPU/GPU memory needs, and admission limits.
- Allowed languages, voices, context sizes, input/output duration limits, and optional token budget.
- State scope and persistence guarantees.
- Allowed locality: same-process, local worker, self-hosted cluster, or external provider.
- Code license, weight license, provenance, and artifact checksum.
- Authentication/network requirements, rate limits, and optional cost attributes.

### 8.3 Negotiation rule

At graph planning time the framework MUST compute **effective capabilities**, rather than blindly returning unioned adapter feature lists. The weakest required edge or node determines what the whole pipeline can honestly promise.

Examples:

- Streaming STT + output-streaming LLM + streaming TTS does **not** imply native duplex LLM processing.
- A native voice adapter without tool support does **not** give the pipeline tool calling unless an explicit tool-capable node/sidecar is added.
- A TTS adapter that only accepts complete utterances cannot satisfy a graph requiring token-level live text ingestion.
- A hosted API that uses SSE for output does not automatically accept incremental request bodies in an ongoing inference turn.

On unmet mandatory capabilities, planning MUST fail with actionable incompatibility diagnostics. Optional downgrades MUST be visible to the application and cannot silently broaden data disclosure or tool permissions.

---

## 9. Node and port abstraction

### 9.1 A node is not necessarily a model

Valid node types include:

- **Sources:** microphone, audio file, message stream, queue, document loader.
- **Model nodes:** STT, LLM, TTS, embeddings, audio understanding, native voice.
- **Transforms:** audio resampling, normalization, text segmentation, token buffering, schema parsing.
- **Policies:** endpointing, VAD, turn control, commit gate, routing, confirmation.
- **Tools:** database lookup, local Rust function, MCP bridge, third-party system.
- **Fan-out / fan-in:** multicast, merge, join, align, aggregate.
- **Sinks:** speaker/playback, subtitle renderer, transcript store, telemetry consumer.

### 9.2 Port specifications

Each node descriptor MUST declare:

1. Named input and output ports.
2. Event types and expected modality.
3. Accepted content encodings/formats and timing assumptions.
4. Whether frames arrive incrementally and whether end-of-input markers are required.
5. Whether provisional data and revisions may arrive.
6. Cardinality and concurrency constraints.
7. Whether the port is essential, optional, or control-only.
8. Which events can be dropped/coalesced versus which require reliable delivery.

Conceptually, a node is not merely a function from a complete `Input` to a complete `Output`. It has an asynchronous **open → exchange events → drain/cancel → close** lifecycle, potentially with several live input and output ports.

### 9.3 Static Rust typing vs dynamic pipeline manifests

The project SHOULD support two complementary representations:

- **Programmatic typed graphs:** Rust consumers obtain compile-time assistance for compatible edge types where possible.
- **Declarative graphs:** TOML/YAML/JSON-like specifications are checked against runtime node descriptors before a graph runs.

A typed Rust API does not eliminate the need for runtime checks: exact codec, sample rate, checkpoint, privacy policy, memory footprint, and provider capabilities are often determined by configuration or model artifacts.

### 9.4 Transformations are explicit nodes

A planner MAY suggest `resample_audio`, `buffer_text`, or `commit_segment`, but MUST NOT silently introduce conversions that materially change latency, quality, privacy, or semantics. Every planned transform SHOULD be visible in the execution plan and telemetry.

---

## 10. Pipeline graph specification

A pipeline is a **directed, typed, validated graph**, not just a linear chain. It defines which processing steps exist and how their port streams interact.

### 10.1 Graph declaration

A pipeline definition SHOULD include:

- Stable pipeline identity and schema revision.
- Named nodes with implementation selection constraints or capability requirements.
- Named source and sink interfaces exposed to the application.
- Explicit edges connecting node/port pairs.
- Required capabilities and optional preferences.
- Edge buffering and delivery/backpressure policies.
- Interaction mode: batch, turn-based, continuous, or duplex.
- Privacy/network/tool permissions and resource budgets.
- Optional feedback loop declarations with bounded control rules.

**A pipeline definition is not an execution.** It becomes runnable only after binding concrete adapters, validating compatibility, allocating resources, and producing an execution plan.

### 10.2 Graph validation

The graph planner/compiler MUST verify:

1. Every named node and port exists.
2. Every edge's output and input types are compatible.
3. Media encoding, timebase, and text commitment semantics are compatible.
4. Mandatory model/tool/duplex capabilities are satisfied.
5. Cycles have an explicit bounded feedback strategy.
6. Required source/sink paths are reachable and there are no unresolved mandatory outputs.
7. Per-session resources are available or admission can be denied cleanly.
8. Network and data-class policy permits every selected adapter.
9. Tool side effects are gated as required.
10. Graph lifecycle and cancellation behavior are well defined.

### 10.3 Composition patterns

- **Chain:** one node feeds the next.
- **Fan-out:** one transcript feeds both logging and LLM reasoning.
- **Fan-in:** merge a tool result, user corrections, and model output under a causal ordering policy.
- **Conditional routing:** choose a language-specific STT or fallback implementation.
- **Feedback:** LLM ↔ tool; turn policy ↔ speech output; retriever ↔ evaluator.
- **Parallel processing:** compute subtitles while preparing a voice reply.
- **Subgraph:** expose a complete voice frontend or RAG workflow as a reusable pipeline node, preserving declared ingress/egress contracts.

A feedback edge MUST have a defined scheduler, queue bounds, and termination/iteration rule; unbounded cycles are invalid by default.

### 10.4 Example declarative graph (illustrative only)

```yaml
pipeline: local_voice_assistant
mode: endpointed_conversation
policy:
  provider_locality: self_hosted_only
  side_effecting_tools: require_committed_input
nodes:
  - id: input
    role: audio_source
  - id: vad
    role: speech_endpointing
  - id: stt
    role: speech_to_text
    requires: [incremental_input]
  - id: agent
    role: text_generation
    requires: [output_streaming, tool_calling]
  - id: tools
    role: tool_executor
  - id: tts
    role: text_to_speech
    requires: [output_streaming]
  - id: output
    role: audio_sink
edges:
  - from: input.audio
    to: vad.audio
  - from: vad.audio
    to: stt.audio
  - from: stt.committed_text
    to: agent.user_input
  - from: agent.tool_call
    to: tools.call
  - from: tools.result
    to: agent.tool_result
  - from: agent.committed_text
    to: tts.text
  - from: tts.audio
    to: output.audio
```

This is intentionally **not** a frozen serialization syntax or a Rust API example. In production, source and sink ports, turn control, cancellation, error paths, timestamps, and all port constraints need to be represented by the actual schema.

---

## 11. Streaming semantics: provisional, committed, and irreversible

This is one of the most important architecture boundaries.

### 11.1 Inbound text

STT can report words that it later changes. A pipeline MUST distinguish:

- **Hypothesis:** text subject to later replacement.
- **Revision:** an explicit change to an identified provisional span.
- **Committed span:** an immutable unit downstream nodes may safely treat as stable.
- **Turn completion:** a control boundary declaring a whole input turn complete; distinct from commit of individual spans.

A stable prefix MAY be committed before the entire utterance is complete. A model that cannot handle edits to existing context must only receive appropriately committed spans, or a policy must explicitly restart/reconcile its generation.

### 11.2 Outbound text

An LLM's emitted text tokens are not automatically safe to speak or publish. The runtime distinguishes:

- Generated/tentative text.
- Committed output segments.
- Queued-for-synthesis text.
- Generated but not yet played audio.
- Played audio confirmed by a sink/client acknowledgement.

Speech already played is physically irreversible. A cancellation can discard unplayed output, but it cannot unsay what the user heard.

### 11.3 Default commit rules

- STT hypotheses MAY be used for speculation by explicitly configured, side-effect-free nodes.
- Side-effecting tools MUST require committed user input by default.
- TTS MUST normally consume committed output segments, not arbitrary tentative tokens.
- Speculative TTS MAY synthesize early, but playback MUST be gated on output commitment unless explicitly opted into a lower-integrity mode.
- Cancellation MUST suppress stale unplayed media and uncommitted output from older generations.

### 11.4 Interruption and barge-in

On interruption, the runtime SHOULD:

1. Mark the current generation epoch as superseded.
2. Stop playback and discard its unplayed queue.
3. Signal cancellation to affected node instances.
4. Reject stale text/audio results from superseded epochs at irreversible sinks.
5. Preserve tool audit state and separately reconcile any already executed side effects.
6. Admit the new user input according to the configured turn or duplex policy.

The model/backend may still consume compute after cancellation. **Cancellation of delivery is distinct from cancellation of computation**; capability descriptors and metrics SHOULD reflect both.

### 11.5 Time and causality

Audio frame time, inference execution time, wall time, and playback time are distinct clocks. Event envelopes SHOULD express the timebase explicitly so that transcription spans, tool events, and spoken output can be aligned without assuming source clocks are identical.

---

## 12. Runtime specification — `inference-runtime`

The runtime turns an approved execution plan into running asynchronous nodes and routable streams.

### 12.1 Responsibilities

- Lifecycle of graph runs and node instances.
- Local resource admission (CPU, GPU, memory, concurrent worker slots).
- Scheduling and fair access across executions.
- Typed event routing across bounded queues.
- Backpressure and overload decisions.
- Cancellation, deadlines, drain, and shutdown.
- Health checking, failure propagation, and optional retries.
- Generation epochs, stream sequencing, and trace propagation.
- Instrumentation, replay, and conformance hooks.

The runtime SHOULD avoid committing public contract crates to any one Rust asynchronous executor. Choosing Tokio internally is an implementation detail, not a domain property.

### 12.2 Execution lifecycles

A graph run SHOULD have clear states such as:

`Created → Validating → Preparing → Active → Draining → Completed`

Failure/cancellation branches MUST be explicit, including partial completion. Nodes similarly participate in a lifecycle akin to `describe → prepare → start → exchange → drain/cancel → stop`.

A run MAY be a one-shot transcription, a turn of text generation, or a long-lived realtime stream.

### 12.3 Bounded buffers and backpressure

Every edge MUST specify finite buffering and overload behavior. Recommended defaults by data class:

| Stream | Overload behavior |
|---|---|
| Live audio input | Apply bounded jitter management; pause/cancel or drop only under explicit audio policy. |
| STT hypothesis updates | Coalesce/supersede old hypotheses when safe. |
| Committed text | Deliver or return an explicit failure; never silently discard. |
| LLM tentative deltas | May buffer/coalesce before a commitment boundary. |
| Tool proposals and outcomes | Reliable delivery/correlation with limits; no silent loss. |
| Generated audio | Bounded playback queue; drop stale epochs on interruption. |
| Metrics/telemetry | Separate best-effort channel so it cannot block realtime inference. |

### 12.4 Resource scheduling

Adapters SHOULD expose resource plans, including GPU residency, estimated memory, startup costs, and supported concurrency. Runtime admission must permit `not_enough_vram`, `backend_busy`, and `deadline_unachievable` outcomes rather than letting sessions degrade indefinitely without explanation.

A model worker MAY stay warm across multiple graph runs. This **does not** imply its conversational state or KV cache is reusable unless the adapter explicitly provides that capability.

### 12.5 Control authority

Control decisions such as committing an input turn, authorizing a tool, changing an execution epoch, and selecting a fallback SHOULD be serialized by a clear authority. Concurrent audio/data processing may continue independently, but it must not yield conflicting irreversible decisions.

---

## 13. Optional session facade — `inference-facade`

The facade is useful for voice agents and interactive clients but is not the foundational unit of the library.

### 13.1 Logical session interface

A session conceptually supports:

| Operation | Meaning |
|---|---|
| `open_session` | Select/configure pipeline, capabilities, privacy and interaction mode. |
| `send` | Submit typed text/audio/control input events. |
| `receive` | Subscribe to typed output/progress/tool/lifecycle events. |
| `commit_input` | Finalize a text span or logical user turn. |
| `interrupt` | Cancel or supersede active generation/playback. |
| `describe` | Inspect selected models, graph, capabilities, and degradation. |
| `close` | Drain or cancel processing and release resources. Closing a speech session delivers every event its binding already received, usage included, before the closed event, and nothing the provider sends after it. |

A session MAY internally dispatch many independent HTTP requests or model invocations. Applications see one logical interaction lifetime; model state management remains truthful to each adapter's contract.

### 13.2 Application transports

- In-process Rust calls/channels SHOULD be the initial access method.
- Unix sockets/local IPC MAY support worker isolation and sidecars.
- WebSockets MAY expose a straightforward browser/server session protocol.
- WebRTC MAY be added for audio-centric clients needing transport-level realtime facilities.
- HTTP request/response and SSE MAY be exposed for one-shot or output-streaming applications.

**Transport persistence ≠ model persistence.** The runtime owns the stable interaction interface, not necessarily a persistent native model connection.

### 13.3 Conversation and inference state

The system MUST keep three states distinct:

1. **Application/conversation state:** committed messages, tool results, memory projections, turn history.
2. **Model inference state:** opaque internal caches, streaming acoustic state, or vendor-managed sessions.
3. **Media state:** frame queues, playback progress, clocks, and interruption barriers.

Conversation state can be reconstructed/replayed. Opaque inference state is portable only if the adapter documents compatible export/import semantics.

---

## 14. Reference pipeline topologies

These examples specify **composition patterns**, not particular required models.

### 14.1 A — Text-only generation

```text
Text source ──> Text model ──> Text sink
                    │
                    └─ optional tool call / result feedback
```

Supports traditional complete-input/streaming-output text models. Recommended first proof of lifecycle, event ordering, and cancellation.

### 14.2 B — Cascaded local voice assistant (reference implementation)

```text
Microphone ─> VAD/endpointing ─> Streaming STT ─> Commit gate
                                                       │
                                                       v
                                                Text LLM / Agent
                                                 │         ^
                                            tool call       │ result
                                                 v         │
                                                Tool executor
                                                       │
                                                committed text
                                                       v
                                                Streaming TTS
                                                       │
                                                Audio playback
```

**Strengths:** strongest composability, convenient text-LLM tools, observability, and independent model replacement. **Tradeoffs:** endpointing/segmentation complexity and extra stages. Pipeline stages may overlap, but an ordinary text LLM still needs a sufficiently finalized prompt for each generation.

### 14.3 C — Speech-aware hybrid

```text
Audio ingress ──> Audio-understanding model ──> Text/decisions ──> TTS ──> Audio out
                                │
                           optional tools
```

Useful when audio-native understanding matters but separate voice rendering is desired. Availability of streaming input and tool calling MUST be negotiated separately.

### 14.4 D — Native speech-to-speech

```text
                      +------------------------+
Audio ingress ───────> | Native speech model    | ───────> Audio playback
  (continuous) <────── | multiport / full duplex| <─────── control events
                      +------------------------+
                                  |
                       optional transcript/tools
```

A single multimodal model adapter may produce text and audio concurrently. Its capabilities MUST be exposed directly; a sidecar tool agent is not equivalent to native model tool calling.

### 14.5 E — Experimental duplex text cascade

```text
Audio ──> Streaming STT ──> Duplex text agent/controller ──> Streaming TTS
                                      ↕
                                tool execution
```

The duplex text node MAY be a specially trained simultaneous-input/output model or an orchestrator combining incremental STT with repeated ordinary LLM turns. The execution plan MUST identify which is used.

A specially trained model might implement `READ`, `WAIT`, `WRITE`, `INTERRUPT`, and `CALL_TOOL` decisions. A conventional autoregressive text model does not acquire this capability merely by accepting an SDK stream API.

### 14.6 F — Non-voice examples

```text
Document ─> Parse ─> Chunk ─> Embed ─> Retrieve ─> LLM ─> Answer

Audio ─> STT ─> Translation model ─> TTS ─> Translated speech

Images ─> Vision model ─> Structured result ─> Rules ─> Storage
```

The existence of these graphs is why `inference` should remain a model-and-pipeline framework, not a voice-only library.

---

## 15. Tool calling as a graph-level capability

Tool calling is an **inference and orchestration concern**, not part of speech synthesis or transcription by default.

### 15.1 Tool lifecycle

```text
Model / policy node
   └─ ToolCallProposed
         └─ Schema validation
               └─ Authorization / confirmation
                     └─ Tool executor (Rust / local service / MCP)
                           ├─ ToolCallResult
                           └─ ToolCallFailed
                                 └─ Correlated context reinjection
```

The tool subsystem MUST provide:

- Structured tool identities, schemas, revisions, and validated argument values.
- Call and generation correlation IDs.
- Separate **proposal**, **authorization**, **execution**, and **result** events.
- Read-only, idempotent write, and non-idempotent write classifications.
- Deadlines, cancellation, concurrency limits, and status reconciliation.
- Human confirmation where policy requires it.
- Distinction between tool retries and generation retries.

The framework MUST NOT assume that stopping a model rolls back an already completed database write or external action.

### 15.2 MCP and other integrations

MCP can be integrated through an **adapter**, but the framework's canonical tool contract remains independent of MCP-specific wire formats. Similarly, provider-native function calls are normalized at their adapters.

### 15.3 Duplex/tool concurrency

A tool may execute while a model speaks, while a user continues talking, or while the runtime waits for a committed turn. These are **separate capabilities** and must be expressed through graph scheduling policy rather than implicitly assumed from the presence of streaming.

Default rule: **a speculative or provisional transcript MUST NOT directly initiate side-effecting tool execution.**

---

## 16. Adapter/plugin strategy

### 16.1 Execution forms

A backend adapter MAY run:

1. **In-process Rust:** Rust-native engine or bindings with acceptable isolation.
2. **Native FFI:** C/C++ model runtime through an explicit FFI boundary.
3. **Subprocess:** Python, Rust, or C++ inference worker controlled by local IPC.
4. **Local service:** self-hosted inference server on localhost/LAN/cluster.
5. **Hosted service:** optional external API, approved by deployment policy.

The adapter descriptor and node contract MUST remain consistent across these forms.

### 16.2 Third-party plugin boundary

Do **not** assume arbitrary dynamically loaded Rust trait objects form a stable plugin ABI. For cross-language or independently built plugins, the preferred long-term extension mechanism is a **versioned process/service protocol** with:

- Explicit protocol handshake and supported contract revisions.
- Node/port and capability description.
- Configuration validation and readiness/health negotiation.
- Typed event/control channels, including efficient binary media frames.
- Failure codes, deadlines, draining, and cancellation semantics.
- Optional artifact/weight manifests and integrity validation.

This avoids coupling every plugin to the same Rust compiler and dependency versions.

### 16.3 Adapter conformance is more important than API imitation

Every adapter MUST pass the tests corresponding to its claimed capabilities. API compatibility with OpenAI, Anthropic, or any other provider is not evidence of native streaming input, full duplex, state resumption, or tool execution behavior.

### 16.4 Provider selection and fallback

- Local/self-hosted adapters SHOULD be the default selection policy.
- Hosted adapters MUST be explicitly enabled.
- Moving user data to a remote provider MUST require an applicable egress/data-class policy.
- Automatic cloud fallback MUST be disabled by default.
- Provider switching MAY occur between turns or graph runs; migration of in-flight opaque model state is not assumed.

---

## 17. Self-hosted, open-source integration candidates

These are **candidate adapters**, not required dependencies. The project must independently verify code license, model-weight license, artifact provenance, hardware suitability, and claimed streaming behavior for every pinned release/checkpoint.

| Layer | Candidate | Relevant use | Important qualification |
|---|---|---|---|
| VAD | [Silero VAD](https://github.com/snakers4/silero-vad) | Local speech activity | VAD is not full turn-taking by itself. |
| STT | [Kyutai delayed-streams-modeling](https://github.com/kyutai-labs/delayed-streams-modeling) | Streaming speech recognition | Check checkpoint languages, streaming semantics, and formats. |
| STT/TTS/VAD | [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) | Local model toolkit | Available modes depend on selected model. |
| STT | [whisper.cpp](https://github.com/ggml-org/whisper.cpp) | Local Whisper inference | Chunked transcription is not automatically native streaming token recognition. |
| LLM | [llama.cpp](https://github.com/ggml-org/llama.cpp) | Local model inference and output streaming | Fixed-input text generation must not be labeled native duplex. |
| LLM | [vLLM](https://github.com/vllm-project/vllm) | Higher-throughput self-hosted serving | Serving protocol support is backend/version dependent. |
| TTS | [Kyutai delayed-streams-modeling](https://github.com/kyutai-labs/delayed-streams-modeling) | Streaming TTS | Rust server and implementation choices differ in how they accept input. |
| TTS | [Kyutai Pocket TTS](https://github.com/kyutai-labs/pocket-tts) | Lightweight local speech synthesis | Verify target platform and model/voice licenses. |
| TTS | [Piper](https://github.com/rhasspy/piper) | Local speech synthesis | Review project maintenance and each voice's license; utterance/chunk behavior varies. |
| Native speech | [Kyutai Moshi](https://github.com/kyutai-labs/moshi) | Self-hosted full-duplex audio model | General agent tools are separate from its native speech capability. |
| Model fine-tuning | [Moshi fine-tuning](https://github.com/kyutai-labs/moshi-finetune) | Research/adaptation | Training is intentionally outside core runtime. |

**Licensing example:** Moshi publishes Rust/Python code and model weights under different licenses; its repository describes Rust backend code as Apache-licensed, Python code as MIT, and weights as CC BY 4.0. The framework MUST treat source-code and weight licenses as separate manifest entries. Attribution, redistribution, commercial-use, and other obligations must be checked per artifact.

### 17.1 Reference self-hosted stack

- **Audio frontend:** local VAD and optional resampling/echo handling.
- **STT:** Kyutai streaming STT or a suitable sherpa-onnx checkpoint.
- **LLM:** a locally served Qwen/Llama-family checkpoint through llama.cpp or vLLM, selected according to actual license/hardware compatibility.
- **Tools:** local Rust functions and optional local MCP bridge.
- **TTS:** Kyutai streaming TTS or a suitable lightweight local synthesizer.
- **Native voice experiment:** Moshi behind an adapter boundary.

The runtime does not need to download, bundle, or redistribute any weight file automatically. Model artifact management SHOULD be explicit and reproducible.

### 17.2 Model capability record

For each supported integration, maintain a machine-readable profile with:

- Source project/revision, checkpoint identifier, hashes, and provenance.
- Code license and weight/voice license separately.
- STT/LLM/TTS/multimodal task declarations and feature tests.
- Language, voice, sample format, and codec requirements.
- Input/output streaming and true concurrency evidence.
- Resource profile for target hardware, not a universal performance promise.
- Known limitations, adapter revision, and conformance-suite results.

---

## 18. Future off-the-shelf provider integrations

Hosted providers should be added as **adapters**, not as a replacement for the local-first contract model.

Potential categories:

| Adapter class | Examples | Main design concern |
|---|---|---|
| Hosted text LLM | Anthropic Messages, OpenAI text APIs, Gemini text APIs | Tool schemas, context management, output streaming, request lifecycle. |
| Hosted realtime voice | OpenAI Realtime, Gemini Live | Persistent session/control events, audio framing, function calls. |
| Hosted speech-native streaming | Amazon Nova Sonic | Provider-specific bidirectional protocol and tool lifecycle. |
| Hosted STT | Specialist recognition services | Interim/final transcript semantics, formats, diarization. |
| Hosted TTS | Specialist speech generation services | Chunking, voice IDs, alignment, and interruption. |

### 18.1 Provider-neutral mapping

Provider-specific wire structures MUST be mapped to canonical types at the adapter boundary. For example:

- Anthropic output SSE remains **output streaming**, not incremental input submission within a live inference turn.
- A Realtime provider MAY support bidirectional session input/output but its exact model-level concurrency must still be represented accurately.
- Vendor-specific tool-call scheduling, limitations, and retry semantics must remain observable.

### 18.2 Privacy and policy

The system MUST know when an edge crosses a trust boundary. A self-hosted graph may process raw audio locally but send only committed text to an approved hosted LLM. That is a distinct permitted data flow from sending raw speech to an external realtime provider.

---

## 19. State, context, persistence, and recovery

### 19.1 Three state categories

| Category | Owner | Persistence approach |
|---|---|---|
| Conversation/application state | Session facade or application | Reconstructible committed event history. |
| Opaque inference state | Adapter/model backend | Checkpoint/resume only when explicitly supported. |
| Media/graph execution state | Runtime and media nodes | Ephemeral queues, positions, epochs, optional recovery snapshots. |

A runtime MUST NOT promise that switching between two arbitrary LLM backends preserves their hidden KV cache or native model conversation state.

### 19.2 Delivery and replay

- Ordering is guaranteed per declared source stream, not globally across all sources.
- Remote transports can duplicate or lose packets and may reconnect.
- Event IDs permit deduplication; exactly-once side effects are not guaranteed.
- Committed, auditable events MAY be durably recorded according to policy.
- Replay SHOULD reproduce framework control decisions when inputs and policy versions are fixed, but model outputs may vary without fully controlled inference conditions.

### 19.3 Failure policy

- A failed node MUST surface a typed, actionable error to the run controller.
- Automatically restarting an inference request MAY be safe; retrying an irreversible tool operation may not be.
- Recovery may start a new generation using committed history, but it must mark changes in generation/epoch identity.
- A provider outage cannot silently activate a cloud replacement or disclose additional data.

---

## 20. Observability and evaluation

The framework SHOULD expose metrics at logical boundaries rather than inventing universal performance targets before benchmarks exist.

| Measure | Interpretation |
|---|---|
| Input → first STT hypothesis | Speech recognition responsiveness. |
| Endpoint/commit latency | Time until stable input is ready downstream. |
| Committed input → first LLM text | Reasoning/serving responsiveness. |
| Committed input → first playable audio | Perceived voice-answer responsiveness. |
| Interruption → playback stop | Barge-in behavior. |
| Text-to-audio alignment drift | Voice timing correctness. |
| Tool call latency and failure | Agent external-work cost and reliability. |
| Queue depth, drops/coalescing | Backpressure and overload behavior. |
| CPU/GPU usage and load admission | Deployment efficiency. |
| ASR word error, task success, tool accuracy | End-to-end quality. |

### 20.1 Trace shape

Traces SHOULD correlate session → graph run → node execution → generation → stream events → tool call → media sink acknowledgement. Logging must separate data-bearing content from operational metadata and default to content redaction.

### 20.2 Quality comparisons

Comparing a cascaded agent and a native voice model requires a common test corpus and declared hardware. Track latency, speech quality, tool correctness, interruption fidelity, resource consumption, and privacy constraints rather than treating time-to-first-byte as the sole metric.

---

## 21. Security, privacy, and deployment

### 21.1 Deployment profiles

| Profile | Admitted compute | Default egress |
|---|---|---|
| `offline_strict` | In-process or local workers only | Denied. |
| `self_hosted_lan` | Local machine and approved internal services | Internal allowlist. |
| `self_hosted_cluster` | Approved owned infrastructure | Authenticated internal paths. |
| `hybrid_opt_in` | Approved self-hosted and explicitly selected remote services | Per-provider / per-data-class allowlist. |

### 21.2 Mandatory controls

- Strong identification and authorization for remotely accessible facades.
- Per-tool permission gates, confirmation hooks, and isolation for untrusted execution.
- Per-adapter network allowlists and secret isolation.
- Content retention policies distinguishing raw audio, partial transcripts, committed text, tool inputs/results, and telemetry.
- No raw audio retention or training-data collection by default.
- Model artifact integrity verification and dependency/weight-license inventory.
- Tenant/session quotas and worker resource isolation for server deployments.
- Explicit opt-in and audit for moving local data to hosted providers.
- No automatic execution of arbitrary code or shell commands generated by a model.

### 21.3 Failure and privacy safety

An unavailable local model MUST NOT trigger implicit remote fallback. Retrying, routing, buffering, and telemetry must all respect the same data-classification policy as the original pipeline.

---

## 22. Training and future model research

The framework must make research *possible* without conflating **inference runtime** with **training infrastructure**.

### 22.1 Exportable time-aligned traces

With explicit consent and a suitable retention policy, evaluations or training jobs MAY export:

- Audio frame timing, transcript hypotheses/revisions, and committed spans.
- Output text/audio event timing, commit decisions, and playback acknowledgements.
- Turn-control actions: `READ`, `WAIT`, `WRITE`, `INTERRUPT`.
- Tool proposals, authorization decisions, result timing, and side-effect classification.
- Graph/model/adapter revisions, clock/timebase metadata, and evaluation labels.

These traces can support training or evaluating a turn-taking controller, a specialized duplex-text model, or fine-tuning a native speech model.

### 22.2 Three research tracks

| Track | Training target | Engineering risk |
|---|---|---|
| **A. Controller learning** | When to read, wait, commit, interrupt, or route using conventional models. | Lower. |
| **B. Duplex text model** | Time-aligned simultaneous reading/writing, tool events, causal visibility. | Medium–high. |
| **C. Native speech model** | Domain/voice adaptation of a self-hostable speech-to-speech model. | High. |

Fine-tuning a conventional chat LLM on complete chat transcripts is **not sufficient** to teach true input-during-output behavior. Duplex training needs timing, causality constraints, read/write decisions, and evaluation of responsiveness versus accuracy.

### 22.3 Model integration remains unchanged

If a new trained model satisfies the existing node/duplex contracts and conformance suite, pipeline graph definitions should not require a new architectural layer. The training system produces a model artifact and adapter; `inference` remains the serving/composition system.

---

## 23. Test strategy and acceptance criteria

### 23.1 Contract tests

- STT revisions cannot rewrite committed spans.
- LLM output streaming and input-during-output capabilities are independently verified.
- TTS input completion and audio emission semantics are reflected accurately.
- Named ports reject incompatible modality, media format, or schema without silent coercion.
- Uncommitted text cannot cause a side-effecting tool by default.
- Event IDs, generation IDs, and per-stream sequence constraints are enforced.

### 23.2 Adapter conformance

- Capability declarations match observed behavior.
- Errors, deadlines, cancellation, and finalization propagate correctly.
- Backend disconnects do not leak tasks or revive stale output.
- Audio timestamps and format transformations are valid.
- Provider/session limits, concurrency, and tool-result correlations are respected.
- Artifact provenance and license metadata are available.
- Local-only profiles prohibit external networking.

### 23.3 Graph/runtime tests

- Detect incompatible ports before execution.
- Reject unbounded cycles and disallowed data flows.
- Fan-out and fan-in preserve their declared ordering/merge policy.
- Backpressure remains bounded under slow consumers.
- Canceled generations cannot produce newly played stale audio.
- Tools do not execute twice merely due to a transport reconnect.
- Graph runs drain and release worker resources.
- Replay reproduces orchestrator control decisions from recorded events.

### 23.4 End-to-end acceptance matrix

| ID | Required result |
|---|---|
| A-01 | A user can depend on only `inference-stt` and one adapter without enabling graph runtime or TTS. |
| A-02 | A user can build and execute a text-only streaming pipeline. |
| A-03 | The same graph API can describe an offline, self-hosted STT → LLM → TTS cascade. |
| A-04 | The local cascade executes with external network access denied. |
| A-05 | A different STT/TTS/LLM adapter can replace its predecessor without changing domain contracts. |
| A-06 | A native speech model can be modeled as one multiport node, not three fake components. |
| A-07 | True duplex and orchestrated incremental output are reported as different effective capabilities. |
| A-08 | STT revisions and LLM speculative output cannot cross irreversible boundaries accidentally. |
| A-09 | Tool proposals are schema-validated, authorized, correlated, and auditable. |
| A-10 | Interruption stops unplayed speech and suppresses stale generations. |
| A-11 | A hosted adapter can be enabled explicitly while the offline profile refuses it. |
| A-12 | An application-facing session can switch among supported topologies without changing its public event vocabulary. |
| A-13 | Graph construction rejects a provider capability mismatch with an actionable error. |
| A-14 | Reusable subgraphs compose without losing cancellation and tracing boundaries. |

---

## 24. Delivery roadmap

| Stage | Priority | Deliverables | Exit condition |
|---|---|---|---|
| **P0 — Vocabulary and contracts** | First | Core, text/audio events, STT/LLM/TTS semantics, testkit | Mock nodes and events pass basic conformance. |
| **P1 — Graph minimum** | First | Typed ports, edge validation, small runtime, single-run lifecycle | A text model or mock transform works in a validated pipeline. |
| **P2 — Local inference vertical slice** | First | One STT, one LLM, one TTS adapter; buffering and cancellation | An entirely offline voice pipeline speaks a response. |
| **P3 — Tools and turns** | Next | Commit barriers, tool node, endpoint controller, safe retries | Local voice agent executes a read-only tool correctly. |
| **P4 — Reusable subgraphs and facade** | Next | Session API, configurable graph definitions, replay/telemetry | Clients switch text-only / voice-cascade configuration transparently. |
| **P5 — Native and hybrid modalities** | Next | Multiport native speech adapter; audio-aware model composition | Native voice graph passes declared capability tests. |
| **P6 — Duplex research** | Later | Incremental-read/write interface, controller or duplex-model adapter | Demonstrated concurrent behavior with measured latency/quality. |
| **P7 — Hosted and scale-out** | Later | Opt-in provider plugins, worker protocol, quotas, privacy auditing | Same contracts run across local and approved remote workers. |

### 24.1 First demonstrator

A deliberately small benchmark/demo SHOULD prove both independent use and composition:

1. Run one self-hosted STT adapter as a standalone library.
2. Run one self-hosted output-streaming LLM adapter as a standalone library.
3. Run one self-hosted TTS adapter as a standalone library.
4. Connect all three through typed pipeline nodes, explicitly committing transcripts and response segments.
5. Add a mock tool executor to verify feedback, cancellation, and correlation.
6. Replace one backend from configuration and pass the same tests.

This is a more valuable initial success criterion than having a large, mostly empty crate inventory.

---

## 25. Architectural decisions

| ID | Decision | Rationale |
|---|---|---|
| ADR-001 | One `inference` Cargo workspace with separate public crates | Unified design with lightweight independent consumption. |
| ADR-002 | Contracts model externally observable behavior, not neural architecture | STT, TTS, LLM, and multimodal backends are implementation-agnostic. |
| ADR-003 | Typed multiport nodes form graphs | Supports chains, tools, forks, joins, native multimodal models, and subgraphs. |
| ADR-004 | Pipelines are declarations; runtime executes validated plans | Clear responsibility and runtime/provider independence. |
| ADR-005 | Sessions are optional, above graph executions | Avoids forcing all users into a voice/session abstraction. |
| ADR-006 | Input, output, and duplex streaming are distinct features | Prevents overclaiming what ordinary inference servers support. |
| ADR-007 | Provisional and committed events are first-class | Critical for accurate STT, TTS, and tool safety. |
| ADR-008 | Tools live outside model adapters | Consistent permission, lifecycle, replay, and correlation. |
| ADR-009 | Self-hosted, open-source integrations are the reference baseline | Privacy, replaceability, offline development. |
| ADR-010 | Hosted providers are opt-in adapter implementations | No core dependence on proprietary platforms. |
| ADR-011 | Cross-language plugins use versioned process protocols | Avoids unstable Rust dynamic ABI and improves isolation. |
| ADR-012 | Training remains a separate workflow using runtime traces | Keeps serving architecture focused while enabling research. |
| ADR-013 | Explicit converters and capability checks | Avoids hidden changes to correctness, latency, and privacy. |

---

## 26. Risks and tradeoffs

| Risk | Mitigation |
|---|---|
| The abstraction becomes so generic it is unusable | Keep domain-specific STT/LLM/TTS contracts with real semantics above a minimal core. |
| Too many crates create needless complexity | Begin with working vertical slices; split crates only at stable API boundaries. |
| Streaming ports are mistaken for true duplex model support | Separate capabilities and include conformance tests for concurrent input/output. |
| STT revisions trigger incorrect agent actions | Immutable committed spans and explicit side-effect barriers. |
| Partial LLM text becomes audibly wrong | Text commit gate and interruption-aware TTS/playback. |
| Graph feedback creates runaway execution | Declare bounded loops, control authority, and termination rules. |
| Non-Rust model workers destabilize the host | IPC/process isolation and managed worker lifecycle. |
| Model licensing surprises users | Per-checkpoint code/weight/license manifest and release-time review. |
| GPU resources become a hidden global bottleneck | Admission budgets, worker descriptors, queue metrics, and fair scheduling. |
| Retrying tools duplicates real-world effects | Idempotency keys, confirmation, status reconciliation; no exactly-once claim. |
| Hosted fallback leaks private data | Default deny outbound inference and per-data-class egress policy. |
| Provider-specific APIs leak into the public surface | Normalize at adapter boundary; namespaced extensions only. |
| Replay expectations exceed model determinism | Guarantee control-event replay separately from exact neural outputs. |

---

## 27. Open design questions

These are intentionally left unresolved at the systems-specification stage:

1. Which contract types should reside in `core` versus `text`, `audio`, and domain crates?
2. Should domain interfaces be asynchronous Rust traits, node descriptors, or two layers with adapters between them?
3. What is the minimum canonical event vocabulary that does not erase STT revisions or TTS timing?
4. How should typed programmatic builders and versioned declarative graph schemas share validation rules?
5. Should graph nodes own their state per execution, per session, or per worker, and how are the scopes declared?
6. What source and playback clock normalization guarantees are realistic across devices and processes?
7. Which event classes need durable logs versus ephemeral in-memory delivery?
8. How should an application control the commit gate and interruption policy without becoming aware of every model detail?
9. What is the reference hardware target for the first self-hosted integration suite?
10. Which weight licenses are permitted by the project's reference installation profile?
11. What are the first two adapters to use when proving that backend replacement is genuinely inexpensive?
12. At what point does the optional process protocol become stable enough for third-party plugin authors?
13. Which features belong in the facade, and which should remain application-level policies?
14. How should downstream applications opt into provisional output and handle rollback/compensation?
15. Which conformance evidence is required before claiming `native_duplex`?

---

## 28. Reference ecosystem and further reading

These are **integration candidates and architecture references**, not endorsements or requirements. Project capabilities and licenses can change; pin exact versions and inspect model cards before adoption.

### Open-source/self-hosted projects

- [Kyutai Moshi](https://github.com/kyutai-labs/moshi) — native realtime speech dialogue; includes a Rust inference backend.
- [Kyutai delayed-streams-modeling](https://github.com/kyutai-labs/delayed-streams-modeling) — streaming STT/TTS implementations, including Rust serving paths.
- [Kyutai Pocket TTS](https://github.com/kyutai-labs/pocket-tts) — lightweight local synthesis option.
- [Kyutai Moshi fine-tuning](https://github.com/kyutai-labs/moshi-finetune) — model adaptation research.
- [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) — local speech inference toolkit.
- [whisper.cpp](https://github.com/ggml-org/whisper.cpp) — self-hosted speech recognition.
- [llama.cpp](https://github.com/ggml-org/llama.cpp) — self-hosted text-generation inference.
- [vLLM](https://github.com/vllm-project/vllm) — self-hosted high-throughput LLM serving.
- [Silero VAD](https://github.com/snakers4/silero-vad) — speech activity detection.
- [Piper](https://github.com/rhasspy/piper) — local TTS project/reference.
- [LiveKit Agents](https://github.com/livekit/agents) — useful reference for voice pipeline and agent composition tradeoffs.
- [Model Context Protocol](https://modelcontextprotocol.io/) — possible tool-adapter integration, not a core dependency.

### Simultaneous input/output research

- [fairseq simultaneous translation](https://github.com/facebookresearch/fairseq/tree/main/examples/simultaneous_translation) — specialized incremental read/write policies, relevant to text duplex research.
- [Moshi paper](https://arxiv.org/abs/2410.00037) — streaming multimodal speech modeling and full-duplex interaction.

---

## 29. Final recommendation

Build **one repository named `inference`** with a small, stable contracts layer and separately usable STT, LLM, and TTS crates. Treat concrete model integrations as adapters, wrap processing units as typed multiport nodes, and compose those nodes through a validated `inference-pipeline` graph. Let `inference-runtime` own execution, backpressure, scheduling, and cancellation, with an optional `inference-facade` for long-lived application sessions.

Start with an **offline, self-hosted text pipeline and voice cascade**. Make provisional/committed events, honest streaming capability negotiation, and adapter conformance correct before building a complex duplex engine. Then incorporate native speech models, agent tools, incremental-text research, and opt-in commercial adapters **without changing the core architecture**.

The intended outcome is not one giant voice framework. It is a **reusable Rust inference ecosystem in which voice agents are one kind of graph assembled from independent, interoperable building blocks**.
