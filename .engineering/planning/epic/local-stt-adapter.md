---
format: aep.planning-md/3
id: epic:local-stt-adapter
kind: epic
status: draft
title: Local STT adapter, standalone
summary: One self-hosted speech-to-text adapter usable with inference-stt alone; declared capabilities match observed.
relations:
- derived_from: architecture-design:inference
- depends_on: epic:contracts-and-testkit
- serves: vision:inference
revision: 2
---
## Context

Step 1 of the first demonstrator: "run one self-hosted STT adapter as a standalone library"
(`docs/design/inference.md:1171`). The STT contract is §7.1 (lines 350-362): batch transcription,
incremental audio ingestion and output streaming as separately declared capabilities; provisional
and committed spans distinct; a chunked offline recogniser wrapped in a streaming node must not
claim native incremental ASR (line 362). Every adapter passes the tests for the capabilities it
claims (line 899) and carries a machine-readable capability record (§17.2, lines 944-952).

Which backend, on which hardware, under which weight licence, is undecided:
`decision-blocker:first-adapters`, `decision-blocker:reference-hardware`,
`decision-blocker:weight-licenses`. The review recommends sherpa-onnx for STT, TTS and VAD in one
toolkit (`docs/design/review-2026-10-18.md`, F6).

## Outcome

One self-hosted STT adapter crate under `adapters/`, usable with `inference-stt` alone, whose
declared capabilities match what the conformance tests observe.

## Scope

- One adapter crate implementing the `inference-stt` contract over the chosen backend, in the
  execution form it needs (§16.1).
- Its capability record: source, checkpoint, hashes, code and weight licence, languages, formats,
  streaming evidence, resource profile (lines 946-952).
- The adapter conformance tests of §23.2 (lines 1114-1120) run against it.

## Exit

- A-01 (line 1137): an example depends on `inference-stt` and this adapter only.
- §23.2: declared capabilities match observed behaviour (line 1114); errors, deadlines and
  cancellation propagate (1115); a backend disconnect leaks no task and revives no stale output
  (1116); the local-only profile prohibits external networking (1120).
- The adapter's `DeclareAdapter` record names `incremental_input` only if the backend accepts
  audio before the utterance ends (line 362).

## Out of scope

A second STT backend (backend-replacement); VAD and endpointing as nodes (tools-and-turns); any
hosted STT.
