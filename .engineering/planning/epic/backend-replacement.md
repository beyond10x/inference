---
format: aep.planning-md/3
id: epic:backend-replacement
kind: epic
status: draft
title: Backend replacement from configuration
summary: A second STT or TTS adapter selected by configuration passes the same scenarios and tests with no contract change.
relations:
- derived_from: architecture-design:inference
- depends_on: epic:local-voice-cascade
- serves: vision:inference
revision: 2
---
## Context

Step 6 of the first demonstrator: "replace one backend from configuration and pass the same tests"
(`docs/design/inference.md:1176`), and A-05: "a different STT/TTS/LLM adapter can replace its
predecessor without changing domain contracts" (line 1141). The design calls this "a more valuable
initial success criterion than having a large, mostly empty crate inventory" (line 1178). Which
second backend is decided with the first ones: `decision-blocker:first-adapters` (the review
recommends whisper.cpp for STT or Piper for TTS, `docs/design/review-2026-10-18.md` F6).

## Outcome

A second adapter for one of STT or TTS, selected by configuration alone, passes the same
conformance scenarios and the cascade's tests as its predecessor, with no change to any contract
crate.

## Scope

- One more adapter crate under `adapters/` for the chosen modality.
- A configuration switch in the cascade example that selects the adapter by binding (`BindNode`).
- The diff gate: the change touches `adapters/` and the example's configuration only.

## Exit

- A-05 (line 1141): `git diff --stat` of the swap shows no path under `crates/`.
- The §23.2 suite and the local-voice-cascade exit tests pass unchanged against the second adapter.
- Both adapters' capability records differ where the backends differ and the cascade's effective
  capabilities (`inference.graph.Pipelines` view) change accordingly.

## Out of scope

A third adapter; hosted adapters; performance comparison between the two (§20.2, later).

## Decisions

Settled on 2026-10-20 (owner took the defaults):

- Backends (`decision-blocker:first-adapters`): sherpa-onnx for STT, TTS and VAD; llama.cpp
  `llama-server` over its local HTTP API for the LLM. The swap is STT, sherpa-onnx to whisper.cpp.
- Hardware (`decision-blocker:reference-hardware`): i9-10900K, 62 GiB, RTX 3090 24 GiB, Linux
  x86_64; tests run on CPU, the GPU is optional.
- Licences (`decision-blocker:weight-licenses`): code Apache-2.0 or MIT, weights CC BY 4.0 or more
  permissive, each checkpoint recorded at adoption; tests read weight paths from environment
  variables and are ignored without them.
