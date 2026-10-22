---
title: Estimate cost from a price table
sidebar_position: 6
description: Give the voice examples a price table you own, read what they estimate, and see where the facade's cost records take over.
lede: No price is looked up for you - you hand in the table, and a count the provider did not report is never priced as zero.
source: examples/realtime-voice (src/usage.rs, examples/talk.rs), examples/repo-podcast/prices, crates/inference-facade (cost.rs, tests/cost.rs)
---

# Estimate cost from a price table

The released voice examples estimate cost from a price table you supply. On main, the session
facade's `cost` feature turns the same idea into cost records priced with
[llm](https://beyond10x.github.io/llm/)'s price book; see the last section.

## The table `talk` and `compare` read

`realtime-voice` reads a JSON table per million tokens:

```json
{
  "name": "<your table name>",
  "date": "<the date the prices are for>",
  "currency": "<currency>",
  "per_million_tokens": {
    "text_in": "<price>",
    "cached_in": "<price>",
    "text_out": "<price>",
    "audio_in": "<price>",
    "audio_out": "<price>"
  }
}
```

Replace each `"<price>"` with a JSON number, at least zero, with at most six decimals. `cached_in`
is optional; without it, cached tokens are priced at their own rate. A missing or unknown field is
refused.

```sh
cargo run -p realtime-voice --example talk -- --provider openai --prices prices.json
```

Each response's estimated cost is printed with its usage, and the total at the end, with the
table's name and date. A count the provider did not report prints as `?`, and no cost is estimated
from a count that is not known.

The table is read before anything opens. A table in another form is refused, naming the field:

```sh
cargo run -q -p realtime-voice --example talk -- --provider openai --prices examples/repo-podcast/prices/gpt-live-1.json
```

```text
error: the price table examples/repo-podcast/prices/gpt-live-1.json: the price table has an unknown field per_session_minute
```

## The tables repo-podcast ships

`repo-podcast` keeps its own price tables in
[`examples/repo-podcast/prices/`](https://github.com/beyond10x/inference/tree/main/examples/repo-podcast/prices):
`gpt-realtime-2.1.json`, in the per-million-token form above, and `gpt-live-1.json`, a price per
session minute, since GPT-Live bills its voice in seconds. Each names its source and the date it
was read; they are list prices as of that date, not a price service. The podcast's running cost
counter reads them.

## Cost records in the facade

On main, in the next release, behind the `inference-facade` feature `cost`:

- `Pricing` holds an `llm-cost` `PriceBook`, built from a versioned price document (revision,
  source, as-of time, currency, digest), and a `PriceBinding` per adapter that names the quantity
  kinds the adapter is charged for and the price that charges them: a model price for the four
  token kinds, a compute price for `local_compute_ms`.
- `cost::price` prices one usage record, with or without a session, so a caller of a native speech
  model can price its records too.
- `LiveSession::price_with` prices every usage record a live session reads, and the records it read
  before; `SessionUsage::cost` holds the cost records and their total.

An adapter with no binding gets a `price_unknown` line per quantity, never skipped and never zero.
A record whose counts are not final gives a lower bound marked `usage_incomplete`. A price document
in another currency than the session's first is refused (`CurrencyMismatch`). The behaviour is held
by
[`crates/inference-facade/tests/cost.rs`](https://github.com/beyond10x/inference/blob/main/crates/inference-facade/tests/cost.rs).
