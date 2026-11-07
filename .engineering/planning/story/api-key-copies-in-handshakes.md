---
format: aep.planning-md/3
id: story:api-key-copies-in-handshakes
kind: story
status: implemented
title: Hosted WebSocket adapters limit and document the copies of the API key
tags:
- from-finding
relations:
- informed_by: review-result:security-deepgram-stt-pass-1
- serves: vision:inference
scope:
- confidence: inferred
  path: adapters/inference-cartesia/src/connection.rs
- confidence: inferred
  path: adapters/inference-cartesia/src/lib.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/connection.rs
- confidence: inferred
  path: adapters/inference-deepgram/src/lib.rs
- confidence: inferred
  path: adapters/inference-deepslate/src/connection.rs
- confidence: inferred
  path: adapters/inference-deepslate/src/lib.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/src/connection.rs
- confidence: inferred
  path: adapters/inference-elevenlabs/src/lib.rs
- confidence: inferred
  path: adapters/inference-openai/src/client.rs
- confidence: inferred
  path: adapters/inference-openai/src/lib.rs
- confidence: inferred
  path: adapters/inference-phonic/src/duplex.rs
- confidence: inferred
  path: adapters/inference-phonic/src/lib.rs
revision: 16
transitions:
- {from: "draft", to: "proposed", at: "2026-11-07T09:15:07Z", actor: "human:operator", revision: 14, decided_on: {"recorded":{"approval":1}}}
- {from: "proposed", to: "active", at: "2026-11-07T09:15:07Z", actor: "human:operator", revision: 15, decided_on: {"recorded":{"approval":1}}}
- {from: "active", to: "implemented", at: "2026-11-07T10:41:18Z", actor: "human:operator", revision: 16, decided_on: {"recorded":{"test_result":1,"approval":1,"review_outcome":1}}}
---
## Context

The security review of the Deepgram streaming STT adapter (review-result:security-deepgram-stt-pass-1)
found that the hosted WebSocket adapters copy the API key into a non-zeroizing `HeaderValue` and
into tungstenite's request buffer: `adapters/inference-deepgram/src/connection.rs:224`, and the same
pattern at `adapters/inference-elevenlabs/src/connection.rs:46`. The adapters' own `Credential`
types zeroize; these two copies outlive them until the allocator reuses the memory.

## Planned scope

- Build the handshake request so the key's copies are limited to what the WebSocket library needs,
  and say in each adapter's crate doc which copies remain and why.
- Apply the same change to every hosted WebSocket adapter that builds an auth header.

## Acceptance

- Each adapter's crate doc names every copy of the key it makes.
- No adapter keeps a `String` or `HeaderValue` holding the key in a struct field past the handshake.
