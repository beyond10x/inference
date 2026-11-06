---
format: aep.planning-md/3
id: story:llama-server-loopback-only
kind: story
status: draft
title: The llama-server adapter accepts loopback hosts only, and local refusals carry no userinfo
tags:
- from-finding
relations:
- informed_by: review-result:full-codebase-2026-11-06
scope:
- confidence: inferred
  path: adapters/inference-llama-server/src/lib.rs
- confidence: inferred
  path: examples/local-voice-cascade/src/lib.rs
- confidence: inferred
  path: examples/local-voice-cascade/src/local.rs
revision: 4
---
## Context

From review-result:full-codebase-2026-11-06 (findings W7, N33, N38), at release 0.16.0:

- W7 `adapters/inference-llama-server/src/lib.rs:202`, `:254-255`: any `http://` base URL is accepted, and the adapter is declared `LocalService`/`LocalWorker` unconditionally, so prompts go in cleartext to a remote server under a local record. Contradicts design §16.4 and the crate doc line 3.
- N33 `examples/local-voice-cascade/src/local.rs:311-315`, `:342-345`: `LocalRefused::NotLoopback` echoes the whole URL, userinfo included, into printed errors.
- N38 `examples/local-voice-cascade/src/lib.rs:721-734`: two commits before any audio pair with the same first frame, inflating one latency.

## Planned scope

- The adapter refuses a base URL whose host is not loopback, with a typed error; the crate doc says so. A remote host is out of scope for this adapter.
- The refusal names scheme, host and port only, never userinfo.
- Pair each commit with the first audio after it that no earlier commit took.

## Acceptance

- `http://10.0.0.5:8080/v1` is refused; `http://127.0.0.1:8080/v1` and `http://[::1]:8080/v1` are accepted.
- A `user:pass@host` URL's refusal text contains neither `user` nor `pass`.
- Two commits before any audio give two distinct latencies.
