---
format: aep.planning-md/3
id: decision-blocker:weight-licenses
kind: decision-blocker
status: open
title: Nobody has decided which weight licences the reference profile permits (Q10)
relations:
- blocks: epic:local-stt-adapter
- blocks: epic:local-llm-adapter
- blocks: epic:local-tts-adapter
revision: 2
---
## Question

Open question 10 of the design: "Which weight licenses are permitted by the project's reference
installation profile?" (`docs/design/inference.md:1235`). Code and weight licences are separate
manifest entries (line 929); the project verifies licence, provenance and hardware suitability for
every pinned checkpoint (line 913); the capability record carries both (line 947). Nothing in the
repository states a policy.

## What it stops

The three local adapter epics: each pins a checkpoint whose weight licence must be admitted before
the adapter's capability record can be written.

## Default on silence

Code under Apache-2.0 or MIT; weights under CC BY 4.0 or more permissive; each checkpoint verified
at adoption and recorded in the adapter's capability record. Anything else is a per-checkpoint
decision for the owner.

## Clears when

The owner states the policy, or accepts the default, and it is written into this body and into
`AGENTS.md`.
