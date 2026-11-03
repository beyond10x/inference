---
format: aep.planning-md/3
id: review-result:adversary-realtime-live-stall-bound-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: the Realtime and GPT-Live read bound holds across reads, and the system-role item'
relations:
- reviews: story:realtime-live-stall-bound-across-reads
- reviews: story:realtime-system-item
revision: 1
---
## Adversary pass 1 on story:realtime-live-stall-bound-across-reads and story:realtime-system-item (U3, head 06c715c)

The adversary's report, as returned (header and findings; prose condensed to the measured parts):

```
unit: story:realtime-live-stall-bound-across-reads, story:realtime-system-item, head 06c715c plus uncommitted test files
verdict: NEEDS-CHANGE
cases: executed 25→31, red 2
origin: introduced 0, pre-existing 0, undecided 1
wrote-outside-worktree: scratch logs only
needs-coordinator: yes
```

- `adversary_a_read_timeout_of_duration_max_does_not_panic`: RED, tokio panics "overflow when
  adding duration to instant" (`adversary16_u3_stall.rs:285`).
- `adversary_live_a_read_timeout_of_duration_max_does_not_panic`: RED, the same panic
  (`adversary16_u3_live_stall.rs:214`).
- GREEN and kept as regression guards: a binary message moves the stall bound (Realtime and
  GPT-Live), the bound counts the time before the first read, and a buffered message behind pings
  is returned after the bound.
- Attacked and not broken: the SystemText wire shape for `create_item` and response `input`,
  the reset after Stalled, pings not moving the bound, cancel safety of the last-message instant
  under dropped reads, the receiver half keeping the bound across `into_split`, and the close
  deadline.

Coordinator's classification: the overflow predates the unit (the base adds the read timeout to
the current instant at the same place), but it lies on the read bound this story rewrote, so the
wave fixes it in a correction unit together with the stale `DEFAULT_READ_TIMEOUT` rustdoc.

```findings
[
  {"file": "adapters/inference-openai/src/client.rs", "line": 775, "category": "boundary", "severity": "warning", "verdict": "CONFIRMED", "origin": "undecided", "message": "last_message + read_timeout overflows and panics for read_timeout Duration::MAX on RealtimeClient and LiveClient reads (and live.rs:1013 in close); adversary_a_read_timeout_of_duration_max_does_not_panic and adversary_live_a_read_timeout_of_duration_max_does_not_panic are red."},
  {"file": "adapters/inference-openai/src/config.rs", "line": 17, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "The DEFAULT_READ_TIMEOUT rustdoc still says how long next_event waits, not that the bound runs from the last message read across calls."}
]
```
