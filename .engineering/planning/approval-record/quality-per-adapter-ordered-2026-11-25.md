---
format: aep.planning-md/3
id: approval-record:quality-per-adapter-ordered-2026-11-25
kind: approval-record
status: draft
title: Quality measures per adapter, latency and connect time are ordered (2026-11-25)
relations:
- decides: story:quality-per-adapter-and-latency-defined
revision: 1
---
## Decision

On 2026-11-25, after the first quality definition was merged, the owner wrote to the coordinator:
"merge it, then continue also quality checking individual adapters, latency, connect time, etc".

The coordinator read it as the order to extend `quality/` with measures per adapter crate, with
latency and connect time among them, and created
story:quality-per-adapter-and-latency-defined on these words. It ran the moves draft to proposed
to active for that story on them. The pinned commit of Ratchet stays
`f139eccc17a548444f0f4e1dc06520120c28739f`.

## What this does not authorise

No live test, no example and no benchmark is run for this story: a call to a hosted provider is
billed, and that needs the owner's word of its own. A latency or connect-time measure is defined
with its command and carries no baseline until such a run exists. No tag and no release follows
from this record. No gate of the repository is tightened or loosened by the documents.
