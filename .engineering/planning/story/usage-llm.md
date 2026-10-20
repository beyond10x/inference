---
format: aep.planning-md/3
id: story:usage-llm
kind: story
status: implemented
title: 'LLM usage: one usage record per llm turn, on the run event path'
summary: inference-llm turns the usage observation of every turn into a usage record; the LLM node sends it on a usage port; llama-server shows its counts.
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
- depends_on: story:usage-domain
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T11:47:53Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-20T11:47:53Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-20T13:52:28Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Context

Every `llm_core::Model::turn` is one model call. Its `TurnOutcome` carries a `TurnObservation` with
`usage: Option<llm_core::Usage>` (input, output, cached input, cache-creation input and reasoning
output tokens, each Optional), `upstream_model` and `final_usage` (`llm` 0.8.0
`crates/llm-core/src/turn.rs:271-355`). `inference-llm` keeps each outcome in
`GenerationHandle::outcomes` (`crates/inference-llm/src/lib.rs:819-822`, pushed in `turn_ended` at
1111-1141) and emits nothing; a failed turn keeps only the `llm` error. The llama-server adapter is
an `llm_core::Model` run by `Generator` (`adapters/inference-llama-server/src/lib.rs:314-328`), so
its usage arrives the same way. This story is also the first emitter on a graph run, so it fixes
how a usage record travels on the run event path; the STT and TTS stories follow the same form.

## Planned scope

- `inference-llm`: `GenerationEvent::Usage(UsageRecorded)`, one per turn that ended, delivered
  after the events of that turn. Quantities from `llm_core::Usage`: `input_tokens` to
  `input_text_tokens`, `output_tokens` to `output_text_tokens`, `cached_input_tokens` and
  `cache_creation_input_tokens` to the kinds of the same name, each `provider_reported`, and
  `unknown` where the count is None or the observation carries no usage; `requests` is 1,
  `measured`. `final` is `final_usage`; `upstream_model` is the observed one. Start and end are
  measured around the turn. A turn that fails yields a record too, every token count `unknown`,
  so a failed call is still counted as a request. Reasoning tokens stay inside output and are not
  a kind of their own.
- `inference-testkit`: the scripted mock model reports a configurable usage, none by default; the
  LLM model node (`src/llm/node.rs`) sends each record in an envelope of kind
  `inference.usage.UsageRecorded` on an output port named `usage`, filling run, node and session
  from the envelope context it already uses. The port convention: a model node that emits usage
  declares one output port `usage`, and an edge or exposure from it uses an overload policy that
  never drops, so usage keeps the reliability of committed facts, never of telemetry (design 684-688).
  The payload trait the node is generic over gains a usage constructor, implemented by the text
  pipeline and cascade payloads.
- `adapters/inference-llama-server`: an offline test over a recorded Chat Completions response that
  carries a `usage` object yields the record with those counts, and one without `usage` yields
  `unknown`; `local_compute_ms` stays `unknown` for this out-of-process server, whose wall time
  the record start and end already carry.

## Domain relations

- UsageRecord to Run, Node and Session, each zero or one, and to Adapter and Model, each one:
  `ess/domains/usage.yaml`, `inference.usage.UsageRecord`, relations `run`, `node`, `session`,
  `adapter`, `model`, as `story:usage-domain` declares them. Here run and node are always present,
  the adapter is the `Generator` adapter record (inferred from `crates/inference-llm/src/lib.rs:10`,
  a `Generator` pairs one adapter record with one model) and the model is that adapter `model_id`.

## Acceptance

A generation of two turns on the mock model, one reporting usage and one reporting none, yields two
`GenerationEvent::Usage` records with the reported counts as `provider_reported` and the missing
ones as `unknown`, and a pipeline run with the LLM model node delivers both on its exposed `usage`
egress with run and node set.

- The llama-server offline tests pass for a response with and without `usage`.
- A failed turn yields a record with `requests` 1 and every token count `unknown`.
- `task check` passes.
