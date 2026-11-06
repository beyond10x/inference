---
format: aep.planning-md/3
id: review-result:adversary-elevenlabs-agent-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the ElevenLabs agent duplex unit
relations:
- reviews: story:elevenlabs-realtime-agent-adapter
revision: 1
---
unit: 0.17.0 U3 story:elevenlabs-realtime-agent-adapter, unit head d204558; the adversary's cases are commit e17a747 (one test file)
verdict: NEEDS-CHANGE
cases: executed 23→26, red 3
origin: introduced 3 / pre-existing 0 / undecided 0

**Cases** (`adapters/inference-elevenlabs/tests/adversary2_agent_attack.rs`, feature `duplex`): 0 passed, 3 failed.

- `audio_after_its_finished_message_opens_no_response_before_the_users_next_turn`: red. Audio of a message that arrives after its `agent_response` opens a new response ahead of the user's next turn, and the answer to that turn lands on it.
- `the_users_silence_after_a_finished_message_with_a_late_audio_tail_is_no_stall`: red. That stray response stays in progress, so a healthy session fails `Stalled` at a 300 ms idle limit.
- `every_ping_is_answered_while_the_caller_reads_nothing_beyond_the_inbound_queue`: red. 300 pings with nobody reading, 256 pongs.

**Suite**: agent_duplex 21/0, claims_record 2/0; clippy with `duplex` clean.

**Attacked, not broken**: barge-in by `event_id` (equal, lower, higher; two interruptions; one with nothing in progress); correction fallback; format mismatch and unknown formats; the metadata deadline; cancel during the handshake and the metadata wait; close and drop bounds; loopback checks; signed-URL redaction; the 1 MiB bound; odd-length PCM carry; offered and unoffered tool calls; the claims record.

Cargo 19 s, wall about 25 min, peak `target/` 391M.

```findings
[
{"file":"adapters/inference-elevenlabs/src/agent_duplex.rs","line":827,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"agent_response completes the response, so audio of that message arriving after it opens a stray response that swallows the next turn and stalls the session at the idle limit."},
{"file":"adapters/inference-elevenlabs/src/agent_duplex.rs","line":599,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"Pings take inbound queue slots and the task reads only after reserving one, so pings past 256 unread messages go unanswered, against the module doc."},
{"file":"adapters/inference-elevenlabs/src/agent_duplex.rs","line":204,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"A cancel stored after an open succeeded cancels the next unrelated open; documented as intended, no caller found that hits it."}
]
```
