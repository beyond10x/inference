---
format: aep.planning-md/3
id: story:core-model-sync
kind: story
status: draft
title: inference-core follows the model vocabulary changed by the spec fixes
summary: Capability orchestrated_duplex, Model provenance and source_revision, the ModelNotFound error, in inference-core.
relations:
- serves: vision:inference
- decomposes: epic:contracts-and-testkit
scope:
- confidence: cited
  path: crates/inference-core/src/model.rs
- confidence: cited
  path: crates/inference-core/tests
revision: 3
---
## Context

`story:ess-defect-fixes` changed `ess/domains/model.yaml` after `story:workspace-skeleton` had
implemented it in `crates/inference-core/src/model.rs`. The Rust vocabulary now lags the
specification by four declarations (tools-model agent report, 2026-10-19):

- `inference.model.Capability` gains `orchestrated_duplex` (design 52, 400, 1143).
- `inference.model.Model` and `inference.model.DeclareModel` gain `provenance` and
  `source_revision` (design 439, 946).
- `inference.model.DeclareAdapter` gains the refusal `unknown-model` with error
  `inference.model.ModelNotFound {model_id}` (design 99). Deciding it needs a model lookup, which a
  pure constructor in `inference-core` does not have; the error type lives in `inference-core`, the
  lookup in whatever store declares adapters (the testkit's in-memory store first,
  `story:testkit-conformance-target`).
- The views `inference.model.Models` and `inference.model.Adapters` publish more fields (checksum,
  both modality lists, backend, execution form); views are the testkit's, not `inference-core`'s.

The hard-coded variant lists in `crates/inference-core/tests/` do not read `ess/`, so `cargo test`
stays green while the code lags the specification; this story closes the gap.

## Steps

1. `model.rs`: the new Capability variant with its ESS name; the two Model fields and the two
   DeclareModel inputs; the `ModelNotFound` error type; `ModelDeclared` unchanged unless the ESS
   event payload changed.
2. Tests: the variant-list and field-set cases follow `model.yaml` (nine capabilities become ten).

## Acceptance

`cargo test -p inference-core` passes with cases asserting that `Capability::ALL` serializes to
the ten names `ess/domains/model.yaml` lists in order, that a `Model` declared with `provenance`
and `source_revision` carries both, and that `ModelNotFound` serializes as `{"model_id": ...}`.

## Out of scope

The model lookup that decides `unknown-model` (the testkit store); the views.
