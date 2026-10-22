---
format: aep.planning-md/3
id: story:gpt-live-interruptible-claim
kind: story
status: implemented
title: GPT-Live claims interruptible while it refuses cancel and interrupt
relations:
- serves: vision:inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-22T08:57:34Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-10-22T08:57:34Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-10-22T08:57:34Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Context

A documentation review on 2026-10-22 found that the GPT-Live binding claims a capability its own
capability record and the limitations page deny:

- `adapters/inference-openai/src/live_duplex.rs` `claims()` includes `Capability::Interruptible`.
- `Capability::Interruptible` means "Can stop or deactivate work within a declared latency
  contract" (`crates/inference-core/src/model.rs:52-53`).
- The same file's `capabilities()` sets `manual_interrupt: false` and `client_cancel: false`.
- `website/docs/status/limitations.md:40-41` says a caller's cancel and truncation are refused on
  GPT-Live.
- The llama-server binding withholds `interruptible` for a weaker reason: it can close the
  connection but cannot see whether the server stopped (`limitations.md:55-56`).

A planner that admits a node because its model claims `interruptible` would admit GPT-Live where
no caller can stop it.

## Planned scope

- Decide from the code which holds: either GPT-Live can stop work within a declared latency (for
  example by closing the session, with a measured bound), or the claim goes.
- If it goes: remove `Capability::Interruptible` from `claims()`, and update every test, adapter
  record and generated page that lists it.
- If it stays: state the mechanism and the latency in `claims()`'s doc comment, and add a test that
  exercises it.
- One `CHANGELOG.md` line.

## Acceptance

- `claims()` and `capabilities()` of the GPT-Live binding no longer contradict each other: either
  `Interruptible` is absent, or a test named in the doc comment stops a response and checks the
  declared latency.
- `website/docs/status/limitations.md` and the adapters reference agree with `claims()`.
- `task check` exits 0.
