---
format: aep.planning-md/3
id: epic:local-voice-cascade
kind: epic
status: draft
title: Local voice cascade, offline (P2)
summary: STT, LLM and TTS composed through typed nodes with commit gates; speaks a response with network denied; barge-in stops playback.
relations:
- derived_from: architecture-design:inference
- depends_on: epic:graph-minimum
- depends_on: epic:local-stt-adapter
- depends_on: epic:local-llm-adapter
- depends_on: epic:local-tts-adapter
- serves: vision:inference
revision: 3
---
## Context

Stage P2, "local inference vertical slice" (`docs/design/inference.md:1160`) and step 4 of the first
demonstrator: "connect all three through typed pipeline nodes, explicitly committing transcripts and
response segments" (line 1174). The topology is B, the cascaded local voice assistant (§14.2, lines
759-778): microphone, VAD/endpointing, streaming STT, commit gate, text LLM, committed text,
streaming TTS, playback. The commit rules of §11.3 (lines 624-628) and the interruption sequence of
§11.4 (lines 632-641) hold across the whole graph; the run executes with network denied (A-04).

The exit condition the design gives: "an entirely offline voice pipeline speaks a response"
(line 1160).

## Outcome

One pipeline declaration composes the three local adapters through typed nodes with commit gates;
it speaks a response offline, stops on barge-in, and no provisional span or tentative segment
crosses an irreversible boundary.

## Scope

- The commit-gate node (§9.1 policies, line 466) between STT and LLM and between LLM and TTS.
- Endpointing from the STT adapter's own signals or a simple energy VAD; a dedicated VAD node is
  tools-and-turns.
- Barge-in: `SupersedeGeneration`, discard of the unplayed queue, cancellation signalled to node
  instances, stale results rejected at the sink (lines 634-637).
- The offline profile: `DeploymentProfile` `offline_strict` with egress `denied` (line 1046)
  enforced at planning and at run time.
- Audio sink that acknowledges playback (`AcknowledgePlayback`).

## Exit

- A-03 (line 1139): the same graph API describes the cascade.
- A-04 (line 1140): the cascade runs with external network access denied, checked by a test that
  fails the run on any outbound connection attempt.
- A-08 (line 1144): an STT revision after commit and a tentative LLM delta at the TTS gate are both
  refused; the scenarios `inference.stream.Span/state/Committed/refuses/inference.stream.ReviseSpan`
  and `inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis`
  pass with real adapters in the loop.
- A-10 (line 1146): interruption stops unplayed speech and the superseded generation produces no
  newly played audio (§23.3 test 5, line 1128); an authored scenario covers the cross-row discard
  that synthesis cannot arrange.
- §23.3 tests 3 and 7 (lines 1126, 1130): fan-out preserves its declared order; the run drains and
  releases worker resources.
- The §20 measures "committed input to first playable audio" and "interruption to playback stop"
  (lines 1022-1023) are emitted as metrics, not promised as targets.

## Out of scope

Tools and the turn controller (tools-and-turns); a second backend (backend-replacement); sessions.

## Decisions

Settled on 2026-10-20 (owner took the defaults):

- Backends (`decision-blocker:first-adapters`): sherpa-onnx for STT, TTS and VAD; llama.cpp
  `llama-server` over its local HTTP API for the LLM. The swap is STT, sherpa-onnx to whisper.cpp.
- Hardware (`decision-blocker:reference-hardware`): i9-10900K, 62 GiB, RTX 3090 24 GiB, Linux
  x86_64; tests run on CPU, the GPU is optional.
- Licences (`decision-blocker:weight-licenses`): code Apache-2.0 or MIT, weights CC BY 4.0 or more
  permissive, each checkpoint recorded at adoption; tests read weight paths from environment
  variables and are ignored without them.
