---
format: aep.planning-md/3
id: story:openai-transport-deadlines-and-cleartext
kind: story
status: active
title: OpenAI WebSocket transport has a handshake and write deadline and refuses cleartext off loopback
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
- serves: vision:inference
scope:
- confidence: inferred
  path: adapters/inference-openai/src/client.rs
- confidence: inferred
  path: adapters/inference-openai/src/duplex.rs
- confidence: inferred
  path: adapters/inference-openai/src/live_duplex.rs
- confidence: inferred
  path: adapters/inference-openai/tests/gpt_live_duplex.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-06T00:41:02Z", actor: "human:timo", revision: 6}
- {from: "proposed", to: "active", at: "2026-11-06T00:41:02Z", actor: "human:timo", revision: 7}
---
## Context

From review-result:full-codebase-2026-11-06 (findings W1, W2, N7, N8, N37), at release 0.16.0:

- W1 `adapters/inference-openai/src/client.rs:750-756`: the WebSocket opening handshake has no deadline; a server that accepts TCP and never completes TLS or the upgrade hangs `OpenAiRealtime::open` and `OpenAiLive::open`.
- W2 `client.rs:750-754`: a URL that is not `wss://` sends `Authorization: Bearer <key>` in cleartext to any host; the ElevenLabs adapter refuses `ws://` off loopback, this one does not.
- N7 `client.rs:830-835`: socket writes have no deadline; a peer that stops draining blocks every session operation.
- N8 `duplex.rs:1012`, `live_duplex.rs:803`, `:1016`: `server_errors` and `backend_usage` grow without a cap.
- N37 `tests/gpt_live_duplex.rs:1119-1122`: the `native_duplex_verdict` assertion is a tautology.

## Planned scope

- One deadline over the whole opening handshake, and one per socket write, both stated in the crate doc.
- Refuse a non-`wss://` URL whose host is not loopback before connecting, with a typed error.
- Cap `server_errors` and `backend_usage` as the ledger caps `dropped_events`, counting what was dropped.
- Replace the tautology with an assertion an alternative implementation would fail.

## Acceptance

- A scripted server that accepts TCP and never answers the upgrade makes `open` fail within the stated deadline.
- `ws://` to a non-loopback host is refused before any connection; `ws://127.0.0.1` still connects.
- A peer that stops reading makes a session write fail within the stated deadline.
- A server sending more error events than the cap leaves the list at the cap with the overflow counted.
- The rewritten N37 assertion fails against a mutant verdict.
