---
format: aep.planning-md/3
id: story:usage-domain
kind: story
status: implemented
title: Usage records declared in ess/ as inference.usage and carried by inference-core
summary: A UsageRecord with quantity kinds and sources, related to run, node, session, speech session, response, adapter and model, validated and answered by the testkit.
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T11:06:26Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-20T11:06:27Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-20T13:52:25Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Context

`epic:usage-and-cost` introduces a noun no ESS document declares: the usage of one model call.
Today usage exists only inside `inference.duplex` (`TokenCounts`, `ResponseUsage`,
`TranscriptionUsage`, `ess/domains/duplex.yaml:80-115`, recorded by `RecordResponseUsage` and
`RecordTranscriptionUsage`), and `inference-llm` keeps the usage observation of each `llm` turn in
`GenerationHandle::outcomes` without emitting it (`crates/inference-llm/src/lib.rs:819-822`). Guardrail
7 and the AGENTS.md rule "spec first" put the noun in `ess/` before any emitter builds on it.

Decision taken here (reversible, the coordinator to confirm): a new domain `inference.usage` in
`ess/domains/usage.yaml`, not an extension of `inference.model` or `inference.duplex`. Why: the record
references nouns of five domains (graph, session, duplex, model) and belongs to none of their
layers; extending `inference.duplex` would make STT, LLM and TTS usage depend on the native speech
domain, and extending `inference.model` would mix per-call events into the declarative catalogue of
models and adapters, whose entities all hold a single Declared state. The duplex usage types stay
as they are; the domain says how they map.

## Planned scope

- `ess/domains/usage.yaml`, listed in `ess/system.yaml` and `ess/ess-inputs.yaml`, each declaration
  with its `read from:` lines (design 303-320 envelope, 367 and 373 usage, 439 cost attributes,
  645 timebase, 688 telemetry, 1231 open question 7):
  - types: `UsageRecordId` (Uuid); `QuantityKind` enum: `input_text_tokens`, `output_text_tokens`,
    `cached_input_tokens`, `cache_creation_input_tokens`, `input_audio_tokens`,
    `output_audio_tokens`, `input_audio_ms`, `output_audio_ms`, `input_characters`, `requests`,
    `local_compute_ms`; `QuantitySource` enum: `provider_reported`, `measured`, `unknown`; `Quantity`
    struct: kind, source, `value: Optional<Integer>` (none exactly when the source is `unknown`).
    `cache_creation_input_tokens` is one kind beyond the epic list: `llm` reports it apart
    (`llm_core::Usage`) and `llm-cost` prices it apart. Cached and cache-creation tokens are a
    subset of the input tokens, never added to them, as `llm_core::Usage` and `TokenCounts` say.
  - entity `inference.usage.UsageRecord` (single Declared state, as `inference.model` entities):
    `run_id`, `node_id`, `session_id`, `speech_session_id`, `response_id` (each Optional),
    `adapter_id`, `model_id`, `upstream_model: Optional<String>` (the model the provider reported
    serving, which `llm` routing may make differ from the declared model), `started_at_ms` and
    `ended_at_ms` (Unix epoch milliseconds on the `wall_time` timebase of
    `inference.stream.Timebase`), `final: Boolean` (false when the provider marked the counts
    partial, as `llm_core::TurnObservation::final_usage`), `quantities: List<Quantity>`. No
    `provider` field: the provider is the adapter `backend` (`inference.model.Adapter`).
  - the `relations:` entries listed under Domain relations below.
  - command `inference.usage.RecordUsage` with outcomes `unknown-adapter` (`AdapterNotFound`),
    `unknown-model` (`ModelNotFound`) and `recorded`, which creates the record and emits event
    `inference.usage.UsageRecorded`.
  - comments mapping `inference.duplex.ResponseUsage` (text, audio, cached tokens per direction)
    and `TranscriptionUsage` onto quantity kinds, and an `UNMAPPED:` marker for durable retention
    of usage records (design open question 7, line 1231): this epic promises delivery, not storage.
  A draft of this shape was validated with ess 0.60.0 during decomposition: valid, and synthesis
  adds `RecordUsage/outcome/recorded`, `/unknown-adapter` and `/unknown-model` with no refusal beyond
  the three known ones.
- `inference-core`: module `usage` with the Rust form of every type above, `UsageRecorded` with
  its declared name as the envelope `kind`, and a constructor that refuses a quantity whose value
  and source disagree. Contract crates fill adapter, model, timing and quantities; the node that
  places the record on a run fills run, node and session.
- `inference-testkit`: a usage store and conform target (`tests/target/usage.rs`) answering the
  three `inference.usage` scenarios, reading the model store for adapter and model existence, as
  the other targets read theirs; `ess/SKIPPED.md` lists none of them.

## Domain relations

Each is declared by this story as a `references` entry on `inference.usage.UsageRecord`; the record
owns nothing and nothing owns it.

- UsageRecord to `inference.graph.Run`, zero or one (`run_id`): inferable, inferred from
  `ess/domains/stream.yaml` `inference.stream.EventEnvelope` field `run_id` (every routable event
  names one run) and from `ess/domains/duplex.yaml`, whose `ResponseUsageRecorded` and
  `TranscriptionUsageRecorded` carry no run and whose `SpeechSession` declares no run relation; no
  `relations:` entry declares it before this story. A native speech record names no run.
- UsageRecord to `inference.graph.Node`, zero or one (`node_id`): inferable, inferred from
  `EventEnvelope` field `producer`.
- UsageRecord to `inference.session.Session`, zero or one (`session_id`): inferable, inferred from
  `EventEnvelope` field `session_id` (Optional) and the comment on `inference.session.Session`
  field `run_id` in `ess/domains/session.yaml`: the envelope session id is the only run-side trace.
- UsageRecord to `inference.duplex.SpeechSession`, zero or one (`speech_session_id`), and to
  `inference.duplex.Response`, zero or one (`response_id`): inferable, inferred from the fields of
  `TranscriptionUsageRecorded` (session) and `ResponseUsageRecorded` (response) in
  `ess/domains/duplex.yaml`; `inference.duplex.Response` relation `session` gives the one session
  of a response.
- UsageRecord to `inference.model.Adapter`, one (`adapter_id`): inferable from the relations
  `inference.duplex.SpeechSession.adapter` (one) and `inference.graph.Binding.adapter` (one): a
  call is served by the one adapter its session or bound node references.
- UsageRecord to `inference.model.Model`, one (`model_id`): inferable from
  `inference.model.Adapter.model` (one) in `ess/domains/model.yaml`; the record names its adapter
  model.

## Acceptance

`ess specify validate --path ess` accepts `ess/domains/usage.yaml` with the six `references`
entries above, and `cargo test -p inference-testkit --test conform` passes
`inference.usage.RecordUsage/outcome/recorded`, `/outcome/unknown-adapter` and
`/outcome/unknown-model` against the testkit usage store.

- `ess verify conform synthesize` refuses nothing beyond the three known affects cascades.
- `task check` passes.
