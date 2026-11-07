---
format: aep.planning-md/3
id: review-result:security-w25-api-key-copies-pass-1
kind: review-result
status: active
title: 'Security review of the API key copies in handshakes: a blank key is sent'
relations:
- reviews: story:api-key-copies-in-handshakes
revision: 1
---
unit: story:api-key-copies-in-handshakes at 0a09f1e, reviewer tests 852fda5
verdict: CONFIRMED; the change holds, one pre-existing warning
cases: executed 72→73 (inference-elevenlabs), red 1
origin: introduced 0 / pre-existing 1 (plus one introduced doc note)

1. Warning, pre-existing: an empty or blank key is sent as an empty auth header instead of being refused as `Credential(Invalid)` in ElevenLabs (`adapters/inference-elevenlabs/src/connection.rs:43`), Cartesia (`connection.rs:44`) and both OpenAI paths (`client.rs:772`, `speech/mod.rs:490`); Deepgram, Deepslate and Phonic refuse it. Test `security_blank_key.rs::a_blank_key_is_refused_as_invalid_before_any_connection`.
2. Note, introduced: the OpenAI speech doc said the header buffer is zeroized once the head is written; reqwest's pending request keeps a clone sharing that buffer until the response head arrives. No extra copy; only the timing was wrong.

Verified: no copy beyond the documented ones (exact-size prefix buffer, `from_maybe_shared` takes the `Bytes` without copying, one `bytes` version, tokio-tungstenite passes the request on uncloned); the buffer is zeroized on success, failure, timeout and cancel; no struct field holds the key; each crate doc's list matches the code; an invalid key is refused without echo. No adapter was missed. The ElevenLabs signed-URL token is copied without zeroizing but is not the API key.
