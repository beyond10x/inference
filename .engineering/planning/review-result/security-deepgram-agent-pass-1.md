---
format: aep.planning-md/3
id: review-result:security-deepgram-agent-pass-1
kind: review-result
status: active
title: Security review of the Deepgram Voice Agent duplex unit, pass 1
relations:
- reviews: story:deepgram-voice-agent-adapter
revision: 1
---
unit: 0.17.0 U4 story:deepgram-voice-agent-adapter, unit head 3d2b01a; review cases are commit c653855 on review/deepgram-agent-security (two test files)
verdict: CONFIRMED (two invariants not held)
cases: executed 103→108, red 2
origin: introduced 1 / pre-existing 1 / undecided 0

**Cases** (`adapters/inference-deepgram/tests/security2_agent_bounds.rs`, `security2_agent_release.rs`):

- red `assistant_words_held_before_a_reply_stay_within_one_message_bound`: the session held 12582923 bytes of server text for one reply, past the 4194304-byte bound of a single message.
- red `a_secure_base_url_whose_host_reads_as_another_is_refused`: `wss://agent.deepgram.com@127.0.0.1:<port>` was taken, and the handshake connected to the host after `@`.
- green: caller text cannot add fields to `Settings`; hostile function arguments (100k-deep nesting, 2 MiB of non-JSON) are proposed as strings without failure; 20 sessions ended five ways leave no thread or connection behind.

**Invariants held**: the key appears in no error, `Debug`, URL or `Settings`, only in the sensitive `Authorization` header; the scheme is 1-32 letters; CR/LF and blank keys refused; `Zeroizing` key and no new copies beyond the handshake; `ws://` loopback only; TLS validation on; 4 MiB message bound, bounded opening and reads, bounded queues, server text in errors capped at 256 characters; no logging in `src/`.

Cargo 0.6 min, wall about 20 min, peak `target/` 741M.

```findings
[
{"file":"adapters/inference-deepgram/src/agent_duplex.rs","line":997,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"Assistant ConversationText arriving while no reply is open is appended to pending_text without limit, so a server makes the session hold memory the caller cannot drain."},
{"file":"adapters/inference-deepgram/src/config.rs","line":126,"category":"integrity","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"checked_base refuses a user part only for ws://, so a wss:// base URL with a user part is accepted and the key-bearing handshake goes to the host after the @."}
]
```
