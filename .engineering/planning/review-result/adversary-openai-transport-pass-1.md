---
format: aep.planning-md/3
id: review-result:adversary-openai-transport-pass-1
kind: review-result
status: active
title: Adversary pass 1 on the OpenAI transport deadline and cleartext fixes
relations:
- reviews: story:openai-transport-deadlines-and-cleartext
revision: 1
---
unit: 0.18.0 RF-openai story:openai-transport-deadlines-and-cleartext, unit head 46d7eae; the adversary's cases are commit f4f3805 (one test file)
verdict: CONFIRMED (one contract finding; the eight added cases pass)
cases: executed 67→75, red 0
origin: introduced 1 / pre-existing 0 / undecided 0

**Cases** (`adapters/inference-openai/tests/adversary_rf1_transport.rs`, features `duplex,gpt-live`), all green: the handshake deadline over TLS; a slow handshake under the deadline connects and one over it fails at the deadline; a loopback redirect is not followed; 15 more URL shapes refused before the credential is read, `ws://[::1]` still connects; a write deadline through the duplex binding ends the session typed as transport-closed and refuses later appends; the error caps exact at 64 and 65.

**Suite**: adversary_rf1_transport 8, transport_deadlines 9, duplex 33, gpt_live_duplex 25; clippy clean.

**Judgement**: `Endpoint` has only public fields, no `#[non_exhaustive]` and no `Default`; adding `connect_timeout` and `write_timeout` breaks struct-literal construction and exhaustive destructuring outside the crate. No literal exists in the repository.

Cargo 0.5 min, wall about 25 min, peak `target/` 414M.

```findings
[{"file":"adapters/inference-openai/src/config.rs","line":40,"category":"contract-drift","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"Adding public fields connect_timeout and write_timeout to the all-public, non-non_exhaustive Endpoint (no Default) breaks struct-literal construction and exhaustive destructuring outside the crate, against the additive-only rule."}]
```
