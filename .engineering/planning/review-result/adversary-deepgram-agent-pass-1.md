---
format: aep.planning-md/3
id: review-result:adversary-deepgram-agent-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the Deepgram Voice Agent duplex unit
relations:
- reviews: story:deepgram-voice-agent-adapter
revision: 1
---
unit: 0.17.0 U4 story:deepgram-voice-agent-adapter, unit head 3d2b01a; the adversary's cases are commit cb9486f (one test file)
verdict: NEEDS-CHANGE
cases: executed 26→37, red 5
origin: introduced 5 / pre-existing 0 / undecided 0

**Cases** (`adapters/inference-deepgram/tests/adversary2_agent_attack.rs`, features `duplex,keychain`): 6 passed, 5 failed.

- `:129`, `:144`: a cancel issued with no open in progress fails the next open with `Cancelled`.
- `:234`: a server ping sent after 100 unread messages gets no pong within 2 s.
- `:401`: a frame after `AgentAudioDone` opens a response that stays in progress; the session fails `Stalled` at the 1 s idle limit (constructed order).
- `:422`: after the user's `ConversationText`, a `THINK_REQUEST_FAILED` warning and silence, the session fails `Stalled`.

**Suite**: agent_duplex 24/0, claims_record 2/0; clippy with `duplex,keychain` clean.

**Attacked, not broken**: two `AgentStartedSpeaking` in a row, `AgentAudioDone` with no reply, two assistant texts before one reply; a double barge-in without `AgentAudioDone`, the next reply whole; `KeepAlive` with the queue full; a message and a frame at exactly 4 MiB; a non-JSON text counted, not fatal; a tool result for an unknown call id. By reading: the opening order, latency fields as number, string or garbage, settings escaping, a tool answer after its reply was cancelled.

Cargo 16 s, wall about 25 min, peak `target/` 438M.

```findings
[
{"file":"adapters/inference-deepgram/src/agent_duplex.rs","line":186,"category":"concurrency","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"OpenCanceller stores a cancel issued with no open in progress and cancels the next, unrelated open."},
{"file":"adapters/inference-deepgram/src/agent_duplex.rs","line":638,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The connection task reserves an inbound slot before reading, so a server ping behind 32 unread messages gets no pong while the caller reads nothing."},
{"file":"adapters/inference-deepgram/src/agent_duplex.rs","line":970,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"A frame after AgentAudioDone opens a response that stays in progress and the session fails as stalled after the idle limit; the documented order sends no audio after AgentAudioDone."},
{"file":"adapters/inference-deepgram/src/agent_duplex.rs","line":987,"category":"acceptance","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"The user's ConversationText makes the ledger await a reply, so after a non-fatal THINK_REQUEST_FAILED warning and silence the session fails as stalled."}
]
```
