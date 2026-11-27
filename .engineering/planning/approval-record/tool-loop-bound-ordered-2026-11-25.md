---
format: aep.planning-md/3
id: approval-record:tool-loop-bound-ordered-2026-11-25
kind: approval-record
status: draft
title: The tool loop's bound is amended in the wave of 2026-11-24 (2026-11-25)
relations:
- decides: story:tool-loop-bound-counts-rounds-per-input
revision: 1
---
## Decision

On 2026-11-25 the coordinator put to the operator how the bound of the tool loop goes on: ship
`Chat` with the limit of four tool calls per conversation written down, amend the specification
and build the new counting in this wave, or ship `Chat` without tools. The operator decided for
the owner: the specification is amended and the change is built in this wave. Its reason, in
its words: the first option "ships a `Chat` that ends at the fifth tool call of a conversation
and takes a red case out of the branch to do it."

Its conditions:

- The amendment of the specification ("the bound counts replies with calls per input") is its own
  commit, before the code, and the adversary's case stays in the branch as the red case it turns
  green.
- The existing tests of `native_voice` stay unchanged and green; one new test states its bound
  under the new counting, and the specification's text says whether an interruption starts a new
  input.
- One implementor round and one adversary pass on that round.
- If the amendment cannot hold for `native_voice` without changing its behaviour, the work stops
  and the question returns to the operator; the documented limit is not taken as a fallback.

The coordinator created story:tool-loop-bound-counts-rounds-per-input on these words, joined it
to the wave of 2026-11-24, and moved it from draft to proposed to active.

## What this does not authorise

No tag and no release: the release that first carries the changed counting waits for the owner.
No new noun in the specification: an entity, field or relation the amendment would need is a
design decision of its own.
