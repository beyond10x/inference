---
format: aep.planning-md/3
id: approval-record:domain-draft-decisions
kind: approval-record
status: draft
title: Domain drafted from the design with eleven delegated decisions and one UNMAPPED relation
tags:
- owner-delegated
relations:
- decides: executable-system-specification:inference
revision: 1
---
## Which stop

The interview the `ess:specifying` skill holds before a domain is drafted from a design document
with no code. The owner was present and had asked for decisions rather than questions ("decide,
don't ask"), so the recommended answer was taken at each open node and recorded here, the way the
skill's *Without an operator* section describes.

## Reason

`owner-delegated`: the owner's standing instruction to take reversible decisions and report them.

## What was decided

Eleven decisions, D1 to D11, listed with their reasons and the design lines they rest on in the
body of `executable-system-specification:inference` under "Decisions taken while drafting". One
question had no evidence either way and stayed an `UNMAPPED:` marker rather than a guess: the
Session to Run relation in `ess/domains/session.yaml`.

## What the owner would have been asked

| # | question | answer taken |
|---|---|---|
| D1 | does a turn own its spans? | no, a span references its turn |
| D2 | does a generation own its output segments? | yes |
| D3 | is the adapter binding a field on Node or an entity? | an entity, Binding, owned by Pipeline |
| D4 | is the execution plan an entity? | no, an optional field on Pipeline set at planning |
| D5 | what are the overload policies called? | pause, drop, coalesce, fail, drop_stale_epochs |
| D6 | which run states are modelled now? | the design's six plus Failed and Cancelled |
| D7 | how much of the tool-call lifecycle now? | Proposed and the provisional-input barrier |
| D8 | how much of the session lifecycle now? | Opened |
| D9 | is edge capacity an invariant? | yes, capacity > 0 |
| D10 | is run failure a command? | yes, FailRun issued by the runtime |
| D11 | what does a deployment profile hold? | admitted localities and a default egress |

Each is reversible by editing the named file and re-running `ess specify validate --path ess`.
