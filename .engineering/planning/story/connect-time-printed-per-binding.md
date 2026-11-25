---
format: aep.planning-md/3
id: story:connect-time-printed-per-binding
kind: story
status: draft
title: Each live command prints its connect time, and the quality page defines it per binding
tags:
- from-finding
relations:
- serves: vision:inference
revision: 1
---
## Context

`quality/adapters.md` defines latency per binding from the commands the repository has
(story:quality-per-adapter-and-latency-defined). One binding prints its connect time: GPT-Live, in
`adapters/inference-openai/tests/live_gpt.rs`, as the time around the connect call. For every other
binding the connect is inside the first-audio figure a command prints, or not timed at all, and the
library keeps no such duration (`quality/not-measured.md`, "Connect time, per binding").

## Planned scope

- Each live test or live example of a binding prints one line with the time from before the
  connect to the open connection, under one name for all bindings.
- `quality/adapters.md` gets one connect-time entry per binding with its command and clock, and
  the entry leaves `quality/not-measured.md`.
- No run in this story: the commands are billed. A number is recorded only from a run the owner
  orders, as a record under `quality/records/` that names the run.

## Acceptance

- Every binding with a live test or live example has a connect-time entry on
  `quality/adapters.md` whose command prints the named line; the check of the quality documents
  goes red when an entry names a line no source prints.
- No connect-time number stands on a page without a record that names its run.

## Out of scope

- A duration kept by the library and offered to a caller.
- Running a billed command.
