---
format: aep.planning-md/3
id: approval-record:quality-definition-ordered-2026-11-25
kind: approval-record
status: draft
title: The quality definition in Ratchet's formats is ordered (2026-11-25)
relations:
- decides: story:quality-defined-in-ratchet-formats
revision: 1
---
## Decision

On 2026-11-25 at about 01:27 CET the owner ordered, through the operator: "use ratchet to define
quality of b10x/inference - do this in the llm/inference repo". The operator relayed it at 01:29
CET with its reading: the work lands in this repository, as documents in Ratchet's formats written
against Ratchet's specification at a pinned commit, because no Ratchet command works yet; each
measure with its source command, its unit, its direction and its current baseline read from a real
run, and one page that says what is not measured yet; "A measure without a command that produces
it is listed as a gap, not given a number."

The coordinator created story:quality-defined-in-ratchet-formats on this order and ran the moves
draft to proposed to active for it on these words. The pinned commit of Ratchet is
`f139eccc17a548444f0f4e1dc06520120c28739f`.

## What this does not authorise

No wave beside this story is opened by this record. Baselines come from existing CI runs; no load
test and no benchmark is run for them. No gate of the repository is tightened or loosened by the
documents: they describe the bounds the gate already enforces.
