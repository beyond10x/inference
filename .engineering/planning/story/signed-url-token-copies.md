---
format: aep.planning-md/3
id: story:signed-url-token-copies
kind: story
status: active
title: The ElevenLabs agent binding limits and documents the copies of its signed-URL token
tags:
- from-finding
relations:
- informed_by: review-result:security-w25-api-key-copies-pass-1
- decomposes: epic:defects-2026-11-10
- serves: vision:inference
scope:
- confidence: inferred
  path: adapters/inference-elevenlabs/src/agent.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/agent_duplex.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/lib.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/tests/agent_duplex.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/tests/agent_duplex_live.rs
- confidence: inferred
  path: examples/realtime-voice/examples/compare.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:17:27Z", actor: "human:operator", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-10T00:17:28Z", actor: "human:operator", revision: 6}
---
## Context

The ElevenLabs agent binding connects with a signed URL and no API key. Its token is copied,
without zeroizing, into the `Uri` and into tungstenite's request line. story:api-key-copies-in-handshakes
covered API keys only and named this copy in the crate doc; its implementor and the security review
of that story both reported it.

## Planned scope

- `adapters/inference-elevenlabs/src/agent_duplex.rs` and its connection path: hold the signed URL
  in a zeroizing type until the handshake, and limit and document its copies as the API key's are.

## Acceptance

- The crate doc names every copy of the signed-URL token.
- No struct field keeps the signed URL or its token past the handshake.

## Scope

Derived 2026-11-10 by `story-scoper` from `main` at `af93222a`. Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `adapters/inference-elevenlabs`, feature `duplex` — cited (story, Planned scope)
- **Files:** `adapters/inference-elevenlabs/src/agent_duplex.rs`: lines 254-288 (`enum Target`, `url`, `host`, `redact`), 290-310 (`ElevenLabsAgent.target` and `Debug`), 327-349 (`signed`, `of`), 405-431 (`open` passing `&self.target` to `connect`), 516-579 (`connect`, `handshake`), 632-643 (`handshake_error`) — cited
- **Files:** `adapters/inference-elevenlabs/src/lib.rs:56-73`, the "Every copy:" list (57-71, API key only) and the signed-URL paragraph (72-73) — cited
- **Symbols:** `Target::Signed`, `ElevenLabsAgent::target`, `ElevenLabsAgent::signed`, `SignedUrl::expose`, `handshake`, `handshake_error` — cited
- **Also likely:** `adapters/inference-elevenlabs/src/agent.rs:44-46` and `:253-330` (`SignedUrl` docs and type) — inferred
- **Also likely:** `adapters/inference-elevenlabs/tests/agent_duplex.rs`, beside `the_signed_url_token_is_in_no_error_and_no_debug` (`:503`) — inferred
- **Also likely:** `examples/realtime-voice/examples/compare.rs:509-519` and `adapters/inference-elevenlabs/tests/agent_duplex_live.rs:59-63`, only if `ElevenLabsAgent::signed` or its reopen behaviour changes — inferred
- **Also likely:** `CHANGELOG.md` `## [Unreleased]` — inferred
- **Documents:** the crate doc in `lib.rs` and the `SignedUrl` doc in `agent.rs`; nothing under `website/docs` or `ess/` mentions the signed URL — cited
- **Confidence:** high for `agent_duplex.rs` and `lib.rs`; medium for `agent.rs` and the tests
- **Would collide with:** any unit editing `agent_duplex.rs` 254-643, `lib.rs` 56-73, `agent.rs` `SignedUrl`, `tests/agent_duplex.rs`. It does not touch the `ServerMessage::UserTranscript` arm at `agent_duplex.rs:1109-1115` where `InputCommitted`/`InputTranscribed` are built (story:duplex-turn-ids), but both share the file — inferred
- **Test:** `cargo test -p inference-elevenlabs --features duplex --test agent_duplex`; a field-level check may use a new `#[cfg(test)] mod tests` in `agent_duplex.rs`, `cargo test -p inference-elevenlabs --features duplex --lib agent_duplex` — inferred

Defect confirmation on `main` at `af93222a` (by reading):
1. Acceptance 1 does not hold: `lib.rs:56-71` lists only the API key's copies; `lib.rs:72-73` defers to `agent::SignedUrl`, whose docs (`agent.rs:44-46`, `:253-255`) say only "zeroized on drop".
2. Acceptance 2 does not hold: `ElevenLabsAgent.target` (`agent_duplex.rs:293`) keeps `Target::Signed(SignedUrl)` (set at 328-329) for the model's lifetime; `open` only borrows it (425-430).
3. Already holding: the `SignedUrl` buffer is `Zeroizing` (`agent.rs:257`), `Debug` shows only the host, handshake errors are redacted, and `tests/agent_duplex.rs:503` checks the token is in no error and no `Debug`.
