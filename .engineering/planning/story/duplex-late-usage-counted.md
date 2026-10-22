---
format: aep.planning-md/3
id: story:duplex-late-usage-counted
kind: story
status: implemented
title: A usage delta that arrives after its response was recorded is still counted
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
scope:
- confidence: cited
  path: adapters/inference-aws/src/duplex.rs
- confidence: inferred
  path: adapters/inference-aws/tests
- confidence: cited
  path: crates/inference-duplex/src/ledger.rs
- confidence: inferred
  path: crates/inference-duplex/tests
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-21T09:08:56Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-21T09:08:56Z", actor: "human:timo", revision: 5}
- {from: "active", to: "implemented", at: "2026-10-22T07:50:42Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1}}}
---
## Context

A read-only review on 2026-10-21 flagged that `adapters/inference-aws/src/duplex.rs` (`add_usage`)
discards the result of `SessionLedger::response_usage_reported`. Read against the ledger
(`crates/inference-duplex/src/ledger.rs`, `response_usage_reported` and `count`): a response reported
with `UsageTotal::Running` is recorded when the next response begins or the session ends
(`settle_ended`); a usage delta for that response arriving after it was recorded returns
`ResponseNotInProgress` and its quantities are not counted. The adapter drops the error, so those
tokens leave no trace. Inferred from the code, not run: whether Nova Sonic sends a usage event for a
response after the next one began is not known.

Checked and not a defect in the same review: the OpenAI Live adapter's `session_usage` result
(the ledger documents no error), and the OpenAI Realtime adapter's dropped transition results (the
ledger's state stands and refusals are dropped by design).

## Planned scope

- Measure first: a scripted Nova Sonic event stream (the adapter's test transport) with a usage
  delta for response N after response N+1 began. If the delta is counted, record that and close.
- Otherwise a late delta is not lost: either the ledger counts it into a further usage record for
  the response, marked as a correction, or it records the quantities as session usage naming no
  response; the adapter no longer discards the result.

## Acceptance

A usage delta for a response that arrives after the response's usage was recorded appears in the
session's usage records, and the session total counts it once.

- `task check` passes.
