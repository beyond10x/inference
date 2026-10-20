---
format: aep.planning-md/3
id: story:llama-server-adapter
kind: story
status: active
title: 'llama-server adapter: an output-streaming generator over the llm Chat Completions client'
summary: Binding, runtime bridge and capability record for a local llama-server; streaming verified, input during output refused, A-02 on it, live test from env.
relations:
- decomposes: epic:local-llm-adapter
- serves: vision:inference
- depends_on: story:capability-record
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T08:29:10Z", actor: "human:timo", revision: 5}
- {from: "proposed", to: "active", at: "2026-10-20T08:29:10Z", actor: "human:timo", revision: 6}
---
## Context

Step 2 of the first demonstrator (`docs/design/inference.md:1172`): one self-hosted
output-streaming LLM adapter usable with `inference-llm` alone. The backend is llama.cpp
`llama-server` over its local HTTP API (`decision-blocker:first-adapters`), which serves the Chat
Completions wire on a local base URL with no credential.

The epic was written when the `llm` repository was treated as absent. Since
`story:llm-over-b10x-llm` and `story:llm-0-8-0`, `inference-llm` wraps `llm_core::Model` from
`b10x-llm` at tag 0.8.0, and a model is any `llm_core::Model` handed to `inference_llm::Generator`
with an `inference.model.Adapter` record. Read at tag 0.8.0, `llm` already has a client that
targets a local base URL with no credential, so this adapter is a binding, a runtime bridge, the
model and adapter declarations and tests, not a protocol client:

- `llm_chat::ChatClient` implements `Model` over Chat Completions (`crates/llm-chat/src/client.rs`),
  documented for vLLM and "other compatible endpoints" (`crates/llm-chat/src/lib.rs:5-6`).
- `BaseUrl::new` accepts `http://127.0.0.1:<port>/v1` (`crates/llm-providers/src/declaration.rs:15-38`),
  and a binding with `AuthKind::Anonymous` and `BillingKind::SelfHosted` sends no authorization
  header (asserted in `crates/llm-chat/tests/local_endpoint.rs`).
- The stream projection ignores response fields it does not read
  (`crates/llm-chat/src/incoming.rs:3-4`), so extra fields the server adds do not refuse a stream;
  `reasoning_content` becomes a reasoning delta, never assistant text (`incoming.rs:130-139`).

### llm gaps

Reported as `llm` gaps and worked around here in the smallest form; each workaround goes when
`llm` closes the gap.

- The HTTP clients of `llm` must be polled inside a tokio runtime: the transport uses
  `tokio::select!` and `tokio::time::timeout_at` (`crates/llm-http/src/transport.rs:210-213, 307-310`).
  `llm_core::Model` does not say so, and neither `inference-llm` nor the graph runtime uses tokio.
  `llm` offers no `Model` that can be polled from outside a runtime: `BlockingModel` of
  `llm-blocking` blocks the calling thread and is not a `Model`. The adapter wraps the client in a
  `Model` that enters a caller-supplied multi-thread tokio runtime handle on every poll of a turn.
- `ChatClient::new` takes an `Arc<dyn SecretResolver>` even for an anonymous binding, and
  `llm-credentials` 0.8.0 exports no resolver that refuses every reference. The test in `llm`
  defines one (`crates/llm-chat/tests/local_endpoint.rs:36-44`); the adapter carries the same few
  lines.
- The pinned fixtures of `llm-chat` are the hosted service and vLLM (`crates/llm-chat/fixtures/`).
  No llama-server stream is pinned in `llm`, so this repository pins its own.

Not assumed, checked by the live test: whether the llama-server build honours
`max_completion_tokens` and `stream_options.include_usage`, both sent by `llm-chat`
(`crates/llm-chat/src/outgoing.rs:61, 88`). An absent usage counter is accepted by `llm` (its
vLLM fixture has none).

### Candidate checkpoint

Qwen2.5-1.5B-Instruct, file `qwen2.5-1.5b-instruct-q4_k_m.gguf` (1 117 320 736 bytes), source
`https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF`. Read on 2026-10-20: the `LICENSE` file
of that repository is the Apache License 2.0, its card metadata says `apache-2.0`, the published
SHA-256 of the file is `6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e`, at
repository revision `91cad51170dc346986eccefdc2dd33a9da36ead9`. This is a candidate, not an
adoption: `decision-blocker:weight-licenses` requires licence, source URL and hash verified at
adoption, so the implementor re-reads all three at that revision and records the date. A different
checkpoint is allowed only under Apache-2.0, MIT, CC BY 4.0 or more permissive. Backend code
licence: llama.cpp is MIT. Tests download no weights and start no server.

## Domain relations

- `inference.model.Adapter -> inference.model.Model`: one adapter references one model; the model
  stands on its own. Inferable from `ess/domains/model.yaml`, entity `inference.model.Adapter`,
  relation `model` (references, cardinality one, via `model_id`). The adapter record references
  the model record that carries the checkpoint, the two licences and the checksum.
- `inference.stream.Generation -> inference.stream.OutputSegment`: one-to-many, the generation
  owns its segments. Inferable from `ess/domains/stream.yaml`, entity `inference.stream.Generation`,
  relation `segments` (owns).
- `inference_llm::Generator -> llm_core::Model`: one generator pairs one adapter record with one
  model and shares it across clones. Inferable (inferred from
  `crates/inference-llm/src/lib.rs:619-665`, `Generator` and `Generator::new`; no ess/1 document
  declares this pairing, it is wrapper code, not a domain relation).
- `inference.model.CapabilityRecord -> inference.model.Adapter` and
  `-> inference.model.Model`: one record per adapter and hardware target, each referencing one
  adapter and one model. Decided by `decision-blocker:capability-record-shape` (cleared
  2026-10-20); no ess/1 document declares it yet. `story:capability-record` declares it in
  `ess/domains/model.yaml`, and this story depends on that story and cites the declaration once it
  exists.

## Planned scope

- `adapters/inference-llama-server` (crate `inference-llama-server`), a workspace member through
  `adapters/*`. It depends on `inference-core`, `inference-llm`, and on `b10x-llm-chat`,
  `b10x-llm-http` and `b10x-llm-credentials` at tag 0.8.0 as workspace dependencies. The dependency
  rule in `AGENTS.md` names these wire crates as allowed in adapters and never in contract crates,
  and its layout row for `adapters/` names the crate.
- Configuration: base URL, served model name, context window and output limit, maximum concurrency
  (the parallel slots of the server, default 1), HTTP limits, and a tokio runtime handle. From it the
  crate builds an anonymous, self-hosted Chat Completions `Binding`, the `ChatClient`, the
  runtime-entering `Model` wrapper and a `Generator`.
- Declarations (design section 17.2, line 944): a `DeclareModel` for the candidate checkpoint
  (family, checkpoint, code licence MIT, weight licence Apache-2.0, checksum, provenance as source
  URL and revision) and a `DeclareAdapter` with backend `llama.cpp llama-server`, execution form
  `local_service`, locality `local_worker`, and capabilities exactly `output_streaming`. It claims
  neither `incremental_input`, `concurrent_input_output`, `native_duplex` nor `interruptible`.
- Capability record, in the shape `story:capability-record` declares, committed as a JSON file in
  the crate: hardware the reference machine of `decision-blocker:reference-hardware`, CPU only;
  evidence for `output_streaming` naming the streaming tests below; resource profile measured by
  hand from one live run (peak resident memory of the server process, model load time, GPU memory
  none), with the llama.cpp build it was measured on; adapter revision. A field with no meaning
  for a text model (audio formats, real-time factor) is left as the declared type allows; if the
  type requires a value, that is reported against `story:capability-record`, not filled with an
  invented one. The testkit helper of that story checks the file against the adapter
  declaration.
- Offline tests against a loopback fixture server replaying pinned llama-server stream bytes,
  written from the documented stream format of the server and marked as such:
  - output streaming verified (section 23.1, line 1106): the fixture holds the rest of the stream
    until the test has received the first `EmitTextDelta`, so a delta is observed before the turn
    ends; the events are `StartGeneration`, then `EmitTextDelta` and `CommitSegment` per delta,
    then `CompleteGeneration`;
  - input during output verified absent (line 1106): input offered while the generation is Active
    is refused with `CapabilityNotClaimed` naming `concurrent_input_output`, and the fixture
    receives no second request;
  - the request the fixture receives: `POST <base>/chat/completions`, no authorization header,
    `stream: true`, `model` the configured name;
  - the adapter declaration lists exactly `output_streaming`, and `Generator::new` accepts it over the
    `Capabilities` of the binding;
  - A-02 (line 1138): the text-only pipeline of source, model node and sink, built as
    `crates/inference-testkit/tests/llm_model_node.rs` builds it, with the generator of this
    adapter in place of the in-process model, runs to Completed against the fixture.
- A live test, `#[ignore]` and behind feature `live`, reading the base URL from
  `INFERENCE_LLAMA_SERVER_URL` and the served model name from `INFERENCE_LLAMA_SERVER_MODEL`. Run
  with `--ignored` while either is unset, it returns at once and prints that it skipped. Against a
  running server it asserts at least two text deltas before completion and a refused offer of
  input, and prints the server build, the usage reported and the wall time. The crate
  documentation says how to start the server with the checkpoint, by hand.

## Acceptance

`cargo test -p inference-llama-server --all-features` passes offline with no server running and no
weights present, including the streaming, input-refusal and A-02 tests named above and the check
of the committed capability record against the adapter declaration; with
`INFERENCE_LLAMA_SERVER_URL` and `INFERENCE_LLAMA_SERVER_MODEL` set against a running
`llama-server`, `cargo test -p inference-llama-server --features live -- --ignored` streams a
generation and refuses input during output.

## Out of scope

Cancellation, failures, deadlines and disconnects (`story:llama-server-cancellation`); tool
proposals (`story:llama-server-tool-proposals`); starting or supervising the server; downloading
weights; vLLM; hosted adapters; the voice cascade.

## Notes from llm

Reply from the `llm` maintainers on 2026-10-20 to the three gaps in this story:

- Runtime: `llm` already has `b10x-llm-blocking`, whose `BlockingModel::new` owns its runtime
  (`llm` `crates/llm-blocking/src/lib.rs:3-11`). Where the adapter calls synchronously, use it
  instead of a caller-supplied runtime. A client without a tokio runtime is filed in `llm` as a draft.
- Credential: `llm` has an `Anonymous` resolver, still private (`llm` `crates/llm-models/src/lib.rs:49-60`);
  keep the local copy until `llm` exports it (filed there as a draft).
- Fixture: a recorded llama-server response is filed in `llm` as a draft; pin our own until then.
