---
format: aep.planning-md/3
id: story:signed-url-token-copies
kind: story
status: draft
title: The ElevenLabs agent binding limits and documents the copies of its signed-URL token
tags:
- from-finding
relations:
- informed_by: review-result:security-w25-api-key-copies-pass-1
revision: 1
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
