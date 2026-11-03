---
format: aep.planning-md/3
id: review-result:adversary-realtime-live-into-split-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: Realtime and GPT-Live clients split into sender and receiver'
relations:
- reviews: story:realtime-live-into-split
revision: 1
---
## Adversary pass 1 on story:realtime-live-into-split (U1, head a3d03e8)

The adversary's report, as returned (header and findings; prose condensed to the measured parts):

```
unit: story:realtime-live-into-split, head a3d03e8 plus uncommitted test files in the working tree
verdict: NEEDS-CHANGE
cases: executed 7→15, red 2
origin: introduced 0 / pre-existing 0 / undecided 2
wrote-outside-worktree: scratch logs only
needs-coordinator: the origin of both red cases
```

- `adversary16_close_after_a_stalled_write_keeps_its_read_timeout_bound`: RED, "close outlived
  its bound: still pending after 10s with a read timeout of 1s, after 7 appends".
- `adversary16_live_close_after_a_stalled_write_keeps_its_documented_bound`: RED, "close outlived
  its documented bound: still pending after 20s, CLOSE_TIMEOUT 15s, after 7 appends".
- Six further cases GREEN and kept as regression guards: the receiver reports `Stalled` while a
  write is pending; a stalled write recovers intact once the peer drains (Realtime and GPT-Live);
  the reunited client returns the events the receiver kept; the sender keeps the input format;
  the receiver reads in its own task and the sender outlives it.
- Suite: `cargo test -p inference-openai --features gpt-live --test split --test gpt_live_split
  --test adversary16_u1_split --test adversary16_u1_live_split`, 13 passed, 2 failed.

Coordinator's classification: both cases are on the story's own path (a write the peer does not
drain, bounded by the caller, then `reunite` and `close`). The base has the same unbounded
close-frame write, so the defect predates the split, but the wave fixes it in a correction unit
on top of the integration branch rather than filing it.

```findings
[
  {"file": "adapters/inference-openai/src/client.rs", "line": 949, "category": "contract-drift", "severity": "warning", "verdict": "CONFIRMED", "origin": "undecided", "message": "RealtimeClient::close (whole or reunited) never returns when the peer stops draining, because the close-frame write in close_socket has no time limit, against its documented Stalled within the read timeout; red case adversary16_close_after_a_stalled_write_keeps_its_read_timeout_bound (adversary16_u1_split.rs:356)."},
  {"file": "adapters/inference-openai/src/live.rs", "line": 1003, "category": "contract-drift", "severity": "warning", "verdict": "CONFIRMED", "origin": "undecided", "message": "LiveClient::close never returns when the peer stops draining, because request_close runs outside the documented max(read_timeout, CLOSE_TIMEOUT) bound; red case adversary16_live_close_after_a_stalled_write_keeps_its_documented_bound (adversary16_u1_live_split.rs:286)."}
]
```
