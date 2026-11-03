---
format: aep.planning-md/3
id: review-result:adversary-realtime-items-and-ping-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on realtime conversation items and the keepalive ping: no findings'
relations:
- reviews: story:realtime-assistant-item-placement
- reviews: story:realtime-live-keepalive-ping
revision: 1
---
## Report

Adversary pass 1 on `impl/realtime-items-and-ping` (diff `6cd4f0f..e2a1fd2`), cases committed
on top as `02ec43f`: `adapters/inference-openai/tests/adversary15_u2_items_and_ping.rs` (8 cases)
and `adapters/inference-openai/tests/adversary15_u2_live_ping.rs` (3 cases, feature `gpt-live`,
one `[[test]]` entry in `adapters/inference-openai/Cargo.toml`). The report as it returned,
header and findings verbatim, prose condensed:

```
verdict: nothing found
cases: executed 82→93, red 0
origin: introduced 0, pre-existing 0, undecided 0
needs-coordinator: no
```

- Cases: exact bytes with both item options and with none; user text through `create_item`
  equals `send_text`; assistant text escaped and round-tripped; a server `error.event_id` comes
  back as `ServerError::event_id`; a pong is skipped and the next event returned; pongs without
  events still stall at the read timeout; payloads of 126, 65536 and 65537 bytes refused as
  `Transport` with nothing written; a ping after the server closed is a `Transport` error; the
  same three for `LiveClient`.
- First isolated run: two ping-after-close cases timed out in the test's own server script; the
  adversary's fix was `drop(client)` before `finish` in its test. Rerun 8/0 and 3/0. Not a
  finding.
- Suite: items_and_ping 7, gpt_live_ping 2, offline 20, gpt_live 13, duplex 28,
  adversary1_realtime 12, lib 0, adversary15_u2_items_and_ping 8, adversary15_u2_live_ping 3,
  all passed; clippy and fmt exit 0.
- Attacked and could not break: 0.9.0 wire shapes, absent versus `null` options, the 125/126
  ping limit (both mutants caught by the unit's own test), the read-timeout doc claim
  (`read_text` uses one fixed deadline, `client.rs:385`), the error variant, `lib.rs` exports.
  The assistant content part type `output_text` is inferred from the provider reference and not
  contradicted by the repository.

```findings
[]
```
