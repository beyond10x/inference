---
format: aep.planning-md/3
id: epic:duplex-research
kind: epic
status: draft
title: Duplex research (P6)
summary: Orchestrated and native duplex-text variants of topology E, reported as different effective capabilities; trace export with consent.
relations:
- derived_from: architecture-design:inference
- depends_on: epic:native-voice
- depends_on: epic:tools-and-turns
- serves: vision:inference
revision: 2
---
## Context

Stage P6, "duplex research" (`docs/design/inference.md:1164`): an incremental read/write
interface, a controller or duplex-model adapter; exit "demonstrated concurrent behaviour with
measured latency/quality". Topology E (§14.5, lines 803-813): the duplex text node may be a trained
simultaneous model or an orchestrator over incremental STT and repeated LLM turns, and the execution
plan must say which. A-07 (line 1143): true duplex and orchestrated incremental output are reported
as different effective capabilities. §22 (lines 1069-1099) keeps training outside the runtime and
exports time-aligned traces with consent.

Blocked by `decision-blocker:native-duplex-evidence` (Q15, line 1240).

## Outcome

An orchestrated duplex controller over the cascade and, where a model exists, a native duplex-text
adapter both run topology E, and the plan reports which one is in use and which effective
capabilities each honestly has.

## Exit

- A-07 (line 1143).
- Time-aligned trace export of §22.1 (lines 1075-1081) behind an explicit consent flag.
- The §20 measures for interruption and alignment drift (lines 1023-1024) emitted for both variants.

## Not decomposed

Stories wait for native-voice and tools-and-turns and for the blocker to clear.
