---
format: aep.planning-md/3
id: approval-record:quality-record-program-ordered-2026-11-25
kind: approval-record
status: draft
title: The program that writes a record is ordered (2026-11-25)
relations:
- decides: story:quality-records-written-by-a-program
revision: 1
---
## Decision

On 2026-11-25 the owner wrote to the coordinator: "And of course we should not only define these
KPIs and metrics and stuff. When that is merged, we also want to evaluate it, right, and track
over time. Maybe you can already start doing that technically. You should also find secrets for
running the adapters in the local keychain."

The coordinator had the measures, their commands and the state of Ratchet read (a scope read, no
run) and took the first story of five from it: the program that writes a record from what a run
left behind. The coordinator ran the moves draft to proposed to active for
story:quality-records-written-by-a-program on the owner's words, and decided that the program is
a subcommand of `crates/inference-docs` and not a new crate.

## What this does not authorise

No call to a hosted provider, no run with a key from a keychain, no local model run and no
benchmark: where keyed runs execute is a question put to the owner on the same day. No workflow
changes and nothing is written to a branch or a ref by a job. No committed record, plan file or
entry state changes. No tag and no release follows from this record.
