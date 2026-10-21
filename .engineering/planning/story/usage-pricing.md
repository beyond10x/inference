---
format: aep.planning-md/3
id: story:usage-pricing
kind: story
status: implemented
title: Cost records from a caller price document behind the facade cost feature, over llm-cost
summary: CostRecord declared in ess; usage priced with llm-cost PriceBook per record and per session, unknowns kept; AGENTS.md admits llm-cost for pricing.
relations:
- decomposes: epic:usage-and-cost
- serves: vision:inference
- depends_on: story:usage-session-totals
- depends_on: story:llm-0-8-3
scope:
- confidence: cited
  path: AGENTS.md
- confidence: cited
  path: crates/inference-facade
- confidence: cited
  path: crates/inference-testkit/src/usage.rs
- confidence: cited
  path: ess/domains/usage.yaml
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-20T22:57:59Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-20T22:57:59Z", actor: "human:timo", revision: 5}
- {from: "active", to: "implemented", at: "2026-10-21T03:04:59Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1}}}
---
## Context

The epic asks for a cost record per usage record and per session when the caller supplies a price
document, with unknown prices and quantities kept unknown, and no provider price lookup or
currency conversion. `llm-cost` in the `llm` repository does exactly that for token and compute
usage: `PriceBook` over a versioned `PriceDocument` (revision, source, as-of time, currency, digest),
`PriceBook::quote` giving a `CostReport` with one `Record` per observation, lines per unit with an
`UnknownReason`, and totals as known subtotal, unknown lines and a complete total only when nothing
is unknown (`llm` 0.8.0 `crates/llm-cost/src/lib.rs`, `src/pricing.rs:1-100`, 140-263). Its units are
input, cached input, cache-creation input and output tokens and resource milliseconds; it has no unit
for audio tokens, audio milliseconds, characters or requests.

AGENTS.md admits the `llm` repository only as `inference-llm`, over `b10x-llm-core`. This story
extends that rule to `b10x-llm-cost`, for pricing only, and says so in AGENTS.md. Why: the price
document format, exact decimal money and unknown-preserving arithmetic already exist and are
versioned there; a second implementation here would fork the price document format. The crate
depends on `b10x-llm-core`, already admitted, and on serde, sha2, toml and thiserror; its `sqlite`
feature (a bundled database) stays off; its licence is Apache-2.0 and its tag is the one
`llm-core` is pinned at, 0.8.0, with the same rust-version. The rule it changes records owner
decisions, so the owner confirms this extension before the story moves to active.

## Planned scope

- Spec first, `ess/domains/usage.yaml`: entity `inference.usage.CostRecord` (single Declared state)
  with `usage_id`, currency, price revision, price digest, lines (quantity kind, quantity, amount as
  an exact decimal String, unknown reason) and totals (known subtotal, unknown lines, complete total
  Optional); command `PriceUsage` with outcomes `unknown-usage` and `priced`; event `CostRecorded`.
  `ess specify validate --path ess`; the new scenarios answered by the testkit usage store.
- `inference-facade` feature `cost`, the only place `b10x-llm-cost` is a dependency (optional,
  workspace dependency at the `llm` tag of `llm-core`, default features): a function pricing one
  `UsageRecord` with a `PriceBook` and a caller-supplied price binding per adapter (model price id,
  serving model id, binding revision; compute price id, hosting provider, account and resource
  class), so a duplex caller with no session prices records too; and a session cost total beside
  the usage total of `story:usage-session-totals`, refusing a price document whose currency differs
  from the one the session started with.
- Mapping: the price binding of an adapter names the quantity kinds that adapter is charged for.
  With a model price, the four token kinds go to an `AttemptUsage`; with a compute price,
  `local_compute_ms` goes to a `ComputeUsage`; each is priced by `PriceBook::quote`, so a missing
  quantity, rate, model or binding match stays unknown with the `llm-cost` reason. A charged kind
  `llm-cost` 0.8.0 has no unit for (audio tokens, audio milliseconds, characters, requests) gets a
  line with reason `price_unknown`. A kind the binding does not charge gets no line. A record whose
  adapter has no price binding gets one `price_unknown` line per quantity it carries: it is priced
  as unknown, never skipped and never zero.
- AGENTS.md: the rule on the `llm` repository names `b10x-llm-cost` behind the facade `cost`
  feature, and the reason above.

## Domain relations

- CostRecord to UsageRecord, one; a usage record has one cost record per price document it is
  priced with, zero without one: inferable, inferred from `llm` 0.8.0
  `crates/llm-cost/src/pricing.rs:71-75` and 83-93 (a `CostReport` is one quote against one price
  document and holds one `Record` per observation); declared by this story as a `references` entry
  `usage` on `inference.usage.CostRecord`.
- UsageRecord to Adapter, one: `ess/domains/usage.yaml`, `inference.usage.UsageRecord`, relation
  `adapter`; the price binding is chosen by that adapter.

## Acceptance

With feature `cost` and a price document with a model price for the mock LLM only, a voice-cascade
session on the testkit mocks yields one cost record per usage record, the LLM record priced line
by line and every line of the STT and TTS records unknown with reason `price_unknown`, and a session
cost total whose known subtotal equals the sum of the priced lines and whose complete total is none.

- A text-only session whose mock LLM reports all four token counts as final, priced with a binding
  that charges only those, gives a complete total equal to the known subtotal, in the document
  currency, naming its revision and digest.
- Without feature `cost`, `cargo tree -p inference-facade` lists no `b10x-llm-cost`.
- `task check` passes.

## Owner decision

The owner confirmed on 2026-10-20 that pricing builds on `llm-cost` ("its perfect, keep going",
replying to the plan that named it): `b10x-llm-cost` at the pinned `llm` tag, behind the facade's
`cost` feature, its `sqlite` feature off. The `AGENTS.md` dependency rule is extended accordingly
by this story.
