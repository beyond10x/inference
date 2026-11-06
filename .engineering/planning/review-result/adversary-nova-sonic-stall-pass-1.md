---
format: aep.planning-md/3
id: review-result:adversary-nova-sonic-stall-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the Nova Sonic stall and bound fixes
relations:
- reviews: story:nova-sonic-stall-and-bounds
revision: 1
---
unit: 0.18.0 RF-aws story:nova-sonic-stall-and-bounds, unit head 5057d1c; the adversary's cases are commit 38a9699 (one test file)
verdict: CONFIRMED (one red case, pre-existing) and one judgement finding NEEDS-CHANGE
cases: executed 124→130, red 1
origin: introduced 1 / pre-existing 1 / undecided 0

**Cases** (`adapters/inference-aws/tests/adversary_rf1_nova_stall_bounds.rs`, feature `duplex`): writer audio after a stall holds off the next stall; a frame after a stall restarts the clock; arrivals hold at their bound (128 to 131) and end with the session; a later block at another rate fails the session; a closed stream fails the session: all green. Red: `an_idle_limit_of_duration_max_still_reports_the_closed_stream` panics at `session.rs:279` ("overflow when adding duration to instant") and the session never hears the stream's end.

**Suite**: every inference-aws target green except that case; clippy clean.

**Attacked, not broken**: the writer's one unbounded failure report runs at most once; `with_open_timeout(Duration::MAX)` is safe; `open` cannot be cancelled mid-way (synchronous, `&mut self`).

Cargo 0.5 min, wall about 12 min, peak `target/` 1.6G.

```findings
[
{"file":"adapters/inference-aws/src/nova_sonic/session.rs","line":279,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"An idle_limit of Duration::MAX, admitted by the ledger, overflows the reader's stall deadline; the reader task panics and the session never hears the stream's end."},
{"file":"adapters/inference-aws/src/nova_sonic/client.rs","line":73,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"N10 named NovaSonic::start and the unit only documented its hang; an additive NovaSonic::with_open_timeout on the private-field client closes it without a contract change."},
{"file":"adapters/inference-aws/src/duplex.rs","line":110,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"OPEN_TIMEOUT's doc says it runs from the connect to the response headers, but it also covers session setup, credential resolution and signing."}
]
```
