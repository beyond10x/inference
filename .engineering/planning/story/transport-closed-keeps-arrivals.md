---
format: aep.planning-md/3
id: story:transport-closed-keeps-arrivals
kind: story
status: draft
title: A send that finds the stream closed still records what already arrived
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
revision: 2
---
## Context

Found during the wave-27 review of story:duplex-late-usage-counted (2026-10-21), read from the
code, not run. In `adapters/inference-aws/src/duplex.rs`, `append_audio` (~922) calls
`transport_closed()` when the command channel is closed, and `send` (~639) does the same when a
send fails; both happen from `submit_tool_result` (~963) as well. `transport_closed()` fails the
ledger at once as `SessionFailureKind::TransportClosed`. Arrivals the binding already received and
has not yet handled in `self.arrivals` (usage deltas, transcript and audio events, a service
error that names the real cause) are then never recorded: their usage is not counted and the
failure reported is TransportClosed even when a waiting arrival carries a more specific error.

Related: the same loss at `close()` (wave-27 finding F1) was fixed in
story:duplex-late-usage-counted: `SessionLedger` now keeps a record for every provider event it
holds, and a close or failure delivers every held event before its end event. Whether that already
covers arrivals still in the binding's `self.arrivals` buffer at a TransportClosed failure is what
the measurement below settles; the fix, if one is needed, uses the same path.

## Planned scope

- Measure first: a scripted Nova Sonic stream (the adapter's test transport) that delivers a
  usage delta, then drops the command channel, followed by `append_audio` and, separately,
  `submit_tool_result`. Record whether the delta is counted and which failure is reported.
- If lost: before failing the ledger as TransportClosed, record the arrivals already received,
  through the same path `next_event` uses; a stream end, client failure or service error among
  them sets the failure instead.

## Acceptance

When a send finds the provider's stream closed, usage the binding received before that point is
in the session's usage records and counted once, and the session's failure is the first failure
among the received arrivals, or TransportClosed when there is none.
