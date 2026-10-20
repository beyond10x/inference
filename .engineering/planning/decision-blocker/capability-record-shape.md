---
format: aep.planning-md/3
id: decision-blocker:capability-record-shape
kind: decision-blocker
status: cleared
title: Nobody has decided the shape of an adapter capability record (resource profile, evidence, languages, formats)
summary: 'ESS leaves the capability record UNMAPPED: claimed vs verified and the resource profile have no shape; the STT epic''s record waits on it.'
relations:
- blocks: epic:local-stt-adapter
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-10-20T07:56:41Z", actor: "human:timo", revision: 3}
---
## Question

What is the capability record of an adapter, as a declared noun, and how does it relate to the
adapter and model it describes and to the hardware it was measured on?

The design asks for "a machine-readable profile" per integration (`docs/design/inference.md:944-952`):
source and revision, checkpoint, hashes, code and weight licence, task declarations and feature
tests, languages and sample formats, streaming and concurrency evidence, a resource profile for
target hardware, known limitations, adapter revision and conformance results. The specification
declares part of it and leaves the rest open:

| part | where it stands |
|---|---|
| source, revision, checkpoint, hash, licences | `inference.model.Model` fields (`ess/domains/model.yaml:74-101`) |
| claimed capabilities, execution form, locality, concurrency | `inference.model.Adapter` fields (`ess/domains/model.yaml:115-158`) |
| evidence that a claim was verified, conformance results | `UNMAPPED` (`ess/domains/model.yaml:134-137`): "a claimed capability and a verified one look the same" |
| resource profile | `UNMAPPED` (`ess/domains/model.yaml:142-144`): "the design lists its parts without a shape or units" |
| languages, audio formats, sample rates, channel limits | not part of the STT contract yet (`crates/inference-stt/src/lib.rs:45-47`) |

The reference-hardware decision says every local adapter states its resource profile against the
reference machine in its capability record, and that a second machine is a second record. That
fixes the record as many per adapter (one per hardware target) but not what owns it, what it
holds, or in which units.

## What it stops

The capability-record part of `epic:local-stt-adapter` ("Its capability record: source,
checkpoint, hashes, code and weight licence, languages, formats, streaming evidence, resource
profile"). The recogniser and its conformance tests are drafted without it: the pinned checkpoint
is declared through `inference.model.Model` and the claims through `inference.model.Adapter`. The
same question stands in front of the capability records of the local LLM and TTS adapter epics.

## Relations to decide

| relation | cardinality | ownership | lifecycle |
|---|---|---|---|
| CapabilityRecord to Adapter | many per adapter, at least one? | owned by the adapter, or standing alone? | does a record outlive a re-declared adapter? |
| CapabilityRecord to Model | one | references | the model stands alone |
| CapabilityRecord to hardware target | one | a field, or an entity of its own? | unknown |
| capability claim to evidence | one claim, one or more named tests? | unknown | does binding a node require a verified claim? |

## Default on silence

Declare `inference.model.CapabilityRecord` in `ess/domains/model.yaml`: references one Adapter and
one Model; fields `hardware` (String, the name of the reference machine), `languages`
(List<String>), `audio_formats` (List<inference.audio.AudioFormat>), `evidence` (one entry per
claimed capability naming the conformance tests that observed it), `resource_profile` (peak
resident memory in MiB, model load time in ms, real-time factor on CPU, GPU memory in MiB or none)
and `adapter_revision`; one record per adapter and hardware target, committed as a JSON file in the
adapter crate and checked by a test that the file parses and matches the declaration of the
adapter. Binding does not require a verified claim yet.

## Clears when

The owner accepts the default or states another shape, and the answer is declared in `ess/` and
written into this body.

## Decision

Taken on 2026-10-20 by the coordinating session under the owner's "decide, don't ask" rule for
choices inside the approved design: the default above. The declaration in `ess/` lands with
`story:capability-record`, which every local adapter story that writes a record depends on.
