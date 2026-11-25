---
format: aep.planning-md/3
id: story:signed-url-token-copies
kind: story
status: implemented
title: The ElevenLabs agent binding limits and documents the copies of its signed-URL token
tags:
- from-finding
relations:
- informed_by: review-result:security-w25-api-key-copies-pass-1
- decomposes: epic:defects-2026-11-10
- serves: vision:inference
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: adapters/inference-elevenlabs/src/agent.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/agent_duplex.rs
- confidence: cited
  path: adapters/inference-elevenlabs/src/lib.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/adversary2_w2_u3_signed_url_memory.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/adversary2_w2_u3_signed_url_rule.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/adversary_w2_u3_signed_url.rs
- confidence: cited
  path: adapters/inference-elevenlabs/tests/agent_duplex.rs
- confidence: cited
  path: examples/realtime-voice/examples/compare.rs
- confidence: cited
  path: website/docs/reference/environment.md
revision: 12
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:17:27Z", actor: "human:operator", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-10T00:17:28Z", actor: "human:operator", revision: 6}
- {from: "active", to: "implemented", at: "2026-11-25T05:11:42Z", actor: "human:operator", revision: 12, decided_on: {"recorded":{"test_result":1,"review_outcome":2,"verification":1}}}
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

As built, 2026-11-25. Commits `161028818`, `1fd3c5a4b` and `ba1870be0`. The scope derived on
2026-11-10 from `main` at `af93222a` is replaced by this.

- `adapters/inference-elevenlabs/src/agent_duplex.rs` (feature `duplex`): a model built with
  `ElevenLabsAgent::signed` gives its signed URL up to the first open that gets past its
  configuration and its runtime. The handshake request's URI is the signed URL's own buffer, and
  the buffer is zeroized when the handshake ends or is dropped. Every later such open fails as
  the new variant `AgentError::SignedUrlSpent` before anything connects. This breaks a caller
  that opened one signed model twice.
- `adapters/inference-elevenlabs/src/agent.rs`: the variant and the `SignedUrl` docs.
- `adapters/inference-elevenlabs/src/lib.rs`: the crate doc lists every copy of the token.
- `adapters/inference-elevenlabs/tests/agent_duplex.rs` and three files of the two adversary
  passes: `adversary_w2_u3_signed_url.rs` (3 cases), `adversary2_w2_u3_signed_url_rule.rs` (3),
  `adversary2_w2_u3_signed_url_memory.rs` (8).
- `examples/realtime-voice/examples/compare.rs`, `website/docs/reference/environment.md`: with
  `ELEVENLABS_AGENT_SIGNED_URL` the ElevenLabs arm measures its first run and records every later
  run with the refusal as its note.
- `CHANGELOG.md`: one `Breaking`, one `Changed` and one `Fixed` entry.

Not built: no change for a public agent's model (`ELEVENLABS_AGENT_ID`), which opens as before.
