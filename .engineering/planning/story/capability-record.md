---
format: aep.planning-md/3
id: story:capability-record
kind: story
status: active
title: Declare the adapter capability record and check it against the adapter's claims
relations:
- decomposes: epic:local-stt-adapter
- serves: vision:inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T08:01:30Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-20T08:01:30Z", actor: "human:timo", revision: 3}
---
## Context

The design asks for one machine-readable profile per integration (`docs/design/inference.md:944-952`).
The specification declares the model and adapter parts and marks verification evidence and the
resource profile `UNMAPPED` (`ess/domains/model.yaml:134-144`). `decision-blocker:capability-record-shape`
settled the shape on 2026-10-20; this story declares it so the local STT, LLM and TTS adapters can
each commit a record.

## Planned scope

- `ess/domains/model.yaml`: `inference.model.CapabilityRecord`, referencing one Adapter and one
  Model; fields `hardware` (String), `languages` (List<String>), `audio_formats`
  (List<inference.audio.AudioFormat>), `evidence` (per claimed capability, the conformance tests
  that observed it), `resource_profile` (peak resident memory MiB, load time ms, real-time factor
  on CPU, GPU memory MiB or none) and `adapter_revision`; one record per adapter and hardware
  target. The two `UNMAPPED` markers it answers go. `ess specify validate --path ess` passes.
- A Rust type for the record in `inference-core` (or the crate that owns `inference.model`), with
  serde, and a testkit helper that loads a record from a JSON file and checks it against an
  adapter's declaration: every claimed capability has evidence, and no evidence names an
  unclaimed one.
- Binding a node does not require a verified claim (unchanged).

## Acceptance

- `ess specify validate --path ess` exits 0 and the synthesized suite's new `CapabilityRecord`
  scenarios pass in the testkit target with `ess/SKIPPED.md` unchanged.
- A testkit test loads a record for the mock STT adapter and passes; a record claiming a
  capability with no evidence, and one with evidence for an unclaimed capability, are refused with
  the capability named.
