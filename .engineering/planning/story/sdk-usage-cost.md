---
format: aep.planning-md/3
id: story:sdk-usage-cost
kind: story
status: draft
title: Every handle reports usage, and cost when a price document is named, never a silent zero
relations:
- decomposes: epic:sdk
- serves: vision:inference
- depends_on: story:sdk-agent
- depends_on: story:sdk-one-call
scope:
- confidence: cited
  path: crates/inference-facade/src/cost.rs
- confidence: cited
  path: crates/inference-facade/tests/cost.rs
- confidence: inferred
  path: crates/inference/src/chat.rs
- confidence: cited
  path: crates/inference/src/recipe.rs
- confidence: inferred
  path: crates/inference/src/stack.rs
- confidence: inferred
  path: crates/inference/src/transcriber.rs
- confidence: inferred
  path: crates/inference/src/usage.rs
- confidence: inferred
  path: crates/inference/src/voice.rs
- confidence: inferred
  path: crates/inference/tests/fixtures/usage-cost.json
- confidence: inferred
  path: crates/inference/tests/usage_cost.rs
- confidence: inferred
  path: ess/domains/sdk.yaml
revision: 14
---
## Context

`docs/design/sdk.md` §7: usage and cost on every handle with no opt-in. The examples carry their own
tallying and pricing (`examples/realtime-voice/src/usage.rs`, 501 lines;
`examples/repo-podcast/src/prices.rs`, 220 lines) beside the facade's
`crates/inference-facade/src/usage.rs` and `cost.rs`. Target T6. Serves O4 and
`epic:usage-and-cost`.

The facade's cost prices only what `llm-cost` has a unit for: "A charged kind `llm-cost` has no unit
for (audio tokens, audio milliseconds, characters, requests) … gets a `price_unknown` line"
(`crates/inference-facade/src/cost.rs:24-27`). `realtime_voice::Tally::cost`
(`examples/realtime-voice/src/usage.rs:271`) prices audio tokens with its own table. Q5 of
`docs/design/sdk.md` §12, default taken: audio lines stay `price_unknown`, never zero, and the
comparison below covers text tokens only.

## Domain relations

No new noun: `inference.usage.UsageRecord` and `CostRecord` (`ess/domains/usage.yaml`). A stack
may name a price document; that key joins `inference.sdk.Stack` as an optional field in this story.

## Planned scope

- `crates/inference/src/usage.rs`: `usage()` on `Agent`, `Chat`, `Transcriber`, `Voice`, returning
  `inference::Usage` (the summed records and the price document the stack names, if any);
  `Usage::cost() -> inference::Result<Cost>` with `Cost: Display`, through the facade's `cost`
  feature. `Cost` (R100) has public fields and no private one:
  - `currency: String`, the price document's;
  - `lines: Vec<CostLine>`, every line of every priced record in record order, each
    `inference_core::usage::CostLine { kind, quantity, amount, unknown }`
    (`crates/inference-core/src/usage.rs:652`): a priced line has `amount: Some(..)` and
    `unknown: None`, an unpriced one `amount: None` and `unknown: Some(UnknownReason::PriceUnknown)`
    (`:612`, serialized `price_unknown`);
  - `total: CostTotal { known_subtotal, unknown_lines, complete_total }` (`usage.rs:668`), added up
    as the facade's `Sum` adds (`crates/inference-facade/src/cost.rs:374-402`), with each record the
    facade refused (`SessionCost::unpriced`, `cost.rs:438`) counted as one more unknown line, so
    `complete_total` is `Some` only when `unknown_lines` is 0.
  Amounts are `llm-cost`'s `Amount` text, trailing zeros trimmed (`llm-cost` 0.8.3 `money.rs:59-63`).
  `Cost`'s `Display`, one example per case the acceptance asserts:
  - complete: `{complete_total} {currency}`, e.g. `0.000864 USD`;
  - incomplete: `at least {known_subtotal} {currency}, {unknown_lines} unknown`, e.g.
    `at least 0 USD, 4 unknown` for a record of four lines none priced; the acceptance asserts the
    prefix `at least 0 USD, ` and the suffix ` unknown`, because the line count is `llm-cost`'s.
  And `impl Display for Usage`, the line the §3.2 program prints with
  `println!("{}", agent.usage())`: `{input} input tokens, {output} output tokens, cost: {cost}`, or
  `…, cost: unknown (no prices)` when no price document is named (or the feature `cost` is off).
  `{input}` sums every input token a record reports (transcription, text and audio; cached tokens
  are part of the input and not added again) and `{output}` every output token; a count no record
  reported, or a sum with an unknown part, prints `unknown`, never 0. Displaying never fails
  (`docs/design/sdk.md` §7). A `pub(crate) fn Usage::from_records(records, book, bindings:
  HashMap<AdapterId, PriceBinding>)` builds a `Usage` from records, a price document and the map
  from each adapter id to the `PriceBinding` the stack built for it (below), for the unit test
  below, and adds no public surface.
- `crates/inference/src/stack.rs`: the `prices` key in the stack file (a path, read relative to the
  file and through the same `Env`), and, behind `cost`, the builder method
  `Stack::prices(self, book: inference::PriceBook) -> Stack` (`PriceBook` re-exported by
  `usage.rs` from the facade, `crates/inference-facade/src/cost.rs:49`), the way an
  application gives a price document in code (`docs/design/sdk.md` §7); a document given so wins
  over the file's, as every builder binding does. Either way the stack turns the book into a
  facade `Pricing` (`crates/inference-facade/src/cost.rs:85-118`) by one rule. A price document
  carries no adapter field: its `models` entries are keyed by price `id` and `serving_model_id`
  (`llm-cost` 0.8.3 `document.rs:23-32`, `deny_unknown_fields`). So for each bound adapter, the
  stack takes the binding's model name, `Stack::model(role)` (story:sdk-stack: the role key
  `model`, `Binding::model`, or the provider's configured model string), and finds the entry whose
  `serving_model_id` equals it. The adapter's `model_id` is a generated UUID
  (`crates/inference-core/src/model.rs:23`, `:170`) and is never matched. A binding with no model
  name is not bound. On a match it calls `Pricing::bind(adapter_id, PriceBinding { .. })`
  (`cost.rs:76-83`):
  - `charges`: the token kinds whose rate the entry carries (`input`, `cached_input`,
    `cache_creation_input`, `output`, each an `Option<Rate>` in `ModelPrice`), because every
    charged kind gets a line and a charged kind with no quote gets `price_unknown`
    (`cost.rs:227-244`); for a `speech` adapter also `InputAudioTokens` and `OutputAudioTokens`, so
    audio stays `price_unknown` (Q5) and is never dropped;
  - `model`: `ModelPriceBinding { price_id: entry.id, serving_model_id, binding_revision:
    entry.binding_revision, billing }` (`cost.rs:52-60`), `billing` being `Metered` for a hosted
    adapter and `SelfHosted` for a local one.
  An adapter with no model name or no matching entry is not bound, so `price` (`cost.rs:154-165`)
  gives each of its records `price_unknown` lines and its cost shows unknown.
- The upstream model. `llm-cost` prices a model line only when the record names the upstream model
  the entry is for: none gives `ModelUnknown`, another gives `ModelMismatch` (`llm-cost`
  `pricing.rs:153-218`; the facade passes `recorded.upstream_model`, `cost.rs:325-341`). The
  facade already has the mechanism, a node wrapper that names the upstream model on each usage
  record that names none, but only as a private test type, `Serving` and `Naming`
  (`crates/inference-facade/tests/cost.rs:161-215`) over the testkit's `CascadePayload`. This
  story lifts it, as the one mechanism, into `crates/inference-facade/src/cost.rs` (behind
  `cost`): `pub struct Serving<P>`, built by `Serving::new(node: Box<dyn NodeImplementation<P>>,
  upstream: String, usage: fn(&mut P) -> Option<&mut UsageRecorded>)`, which wraps each output
  endpoint and sets `upstream_model` with `get_or_insert_with`, so a provider that reports its
  upstream model keeps it. The payload accessor is a function pointer, so the facade names no
  payload type. `crates/inference-facade/tests/cost.rs` drops its own `Serving` and `Naming` and
  uses the lifted type with `CascadePayload`'s usage variant, and its tests pass unchanged.
- `crates/inference/src/recipe.rs`: behind `cost`, where a recipe builds a role's node, a binding
  whose model name matched a price entry (above) has its node wrapped in `Serving` with the matched
  entry's `upstream_model` (`llm-cost` 0.8.3 `document.rs:27`, a required field) as the upstream
  (R99), and the accessor `|p| match p { Payload::Usage(r) => Some(r), _ => None }`, the variant
  story:sdk-payload adds (R98). The stack keeps, per bound adapter, the `upstream_model` of the
  entry it matched, and the recipe reads it there. So an entry prices a binding whose model name
  equals its `serving_model_id`, whatever its `upstream_model`. A binding with no match is not
  wrapped. story:sdk-recipes owns the file; this story comes after it (through story:sdk-agent) and
  adds only that wrapping.
- `crates/inference/src/chat.rs`, `src/transcriber.rs`, `src/voice.rs`: what each handle needs to
  answer `usage()`.
- `ess/domains/sdk.yaml`: the optional price document field on `Stack`.
- `crates/inference/tests/usage_cost.rs` (stub from story:sdk-crate, filled here) and `crates/inference/tests/fixtures/usage-cost.json`
  (new): usage records with text tokens only, a price document and the expected amount. The rates
  are those of `examples/repo-podcast/prices/gpt-realtime-2.1.json` (per million tokens: input 4,
  cached input 0.4, output 24), written in the `llm.prices/1` format the facade reads, because that
  file is not one (it has no `format` and no `models`). The one model entry has `id` `"gpt-realtime-2.1"`,
  `serving_model_id` and `upstream_model` both `"gpt-realtime-2.1"`, `binding_revision` `"1"`,
  and `input`, `cached_input` and `output` rates. Every record states every count the document
  prices: input text tokens 12, cached input tokens 0, cache-creation input tokens 0, output text
  tokens 34, `final` true, and `upstream_model` `"gpt-realtime-2.1"` (what the `Serving` wrapper
  stamps), so the uncached input exists and the total has a `complete_total`. The expected amount is 12 × 4 / 10⁶ + 34 × 24 / 10⁶ = 0.000864 USD, checked
  once at implementation start against `realtime_voice::Tally::cost` and committed. The tests'
  generator reports the same counts: `MockGenerator::reporting([Some(llm_core::Usage {
  input_tokens: Some(12), cached_input_tokens: Some(0), cache_creation_input_tokens: Some(0),
  output_tokens: Some(34), .. })])` (`crates/inference-testkit/src/llm.rs:279`), called "the
  12/34 generator" below, bound as `Binding::llm(..).model("gpt-realtime-2.1")` unless a case
  says otherwise.
- `crates/inference-facade/src/cost.rs` and `crates/inference-facade/tests/cost.rs`: the lifted
  `Serving` above.

## Acceptance


In `crates/inference/tests/usage_cost.rs`, passing in
`cargo test -p inference --features cost --test usage_cost`, over the testkit mocks bound with
`Stack::new().bind(Binding::<role>(<mock>))` (story:sdk-stack; the testkit is a
dev-dependency) and handed to `Agent::voice_with(stack)` or the handles' `new_with(&stack)`
(decision D18 of `api-design:sdk`); no test reads or sets the process environment. The cases bound
by the adapter name `mock` (story:sdk-stack's `registry/mock.rs`; this story adds no registration)
are gated `#[cfg(feature = "testing")]` and run under `--features cost,testing`:

- `Agent`: after a run over `MockSpeechModel::native(<one turn>).closes()` (story:sdk-duplex-node)
  and one over the cascade mocks, `usage()` equals the sum of the `UsageRecorded` events the
  session delivered.
- `Chat::new_with(&stack)`: after two `send`s, `usage()` equals the sum of its session's
  `UsageRecorded` events.
- `Transcriber::new_with(&stack)`: after one `transcribe_file`, `usage()` equals the sum of its
  session's `UsageRecorded` events.
- `Voice::new_with(&stack)`: after one `synthesise`, `usage()` equals the sum of its session's
  `UsageRecorded` events.
- `Agent`, gated `#[cfg(feature = "audio-host")]`: after
  `agent.run_on_devices_with(FakeHost::new(..))` returns over a stack binding `speech` to
  `MockSpeechModel::native(<one turn>).closes()`, `agent.usage()` on the same agent equals the sum
  of the `UsageRecorded` events delivered.
- `usage().cost()` with no price document returns `Error::NoPrices`, never a zero amount.
- `Display`, unpriced: after a `Chat::send` over the 12/34 generator, on a stack naming no price
  document, `chat.usage().to_string()` contains `12 input tokens`, `34 output tokens` and
  `cost: unknown (no prices)`; the whole line is `12 input tokens, 34 output tokens, cost: unknown (no prices)`.
- The `prices` key: the test writes `inference.toml` holding `prices = "prices.json"` and the
  fixture's price document as `prices.json` into a `tempfile::tempdir()`, and builds an `Env` whose
  working directory is that directory. Over
  `Stack::new().bind(Binding::llm(<the 12/34 generator>).model("gpt-realtime-2.1")).resolve_with(&env)?`,
  after one `Chat::send`, `chat.usage().cost()?` has no `price_unknown` line, its total has a
  `complete_total` equal to the fixture's amount, 0.000864 USD, and
  `chat.usage().cost()?.to_string()` is `0.000864 USD` and `chat.usage().to_string()` is
  `12 input tokens, 34 output tokens, cost: 0.000864 USD`.
- `Stack::prices(book)`: the same stack with no file and `.prices(<the fixture's price document
  read as a PriceBook>)` gives the same `Display` line and the same 0.000864 USD.
- The upstream model is stamped: in the priced case above, every `UsageRecorded` the chat session
  delivered from the generator has `upstream_model == Some("gpt-realtime-2.1")`, put there by the
  `Serving` wrapper, as the mock reports none.
- The match is by `serving_model_id`, the stamp is the entry's `upstream_model` (R99): a price
  document whose one entry has `serving_model_id` `"a"` and `upstream_model` `"b"`, and the 12/34
  generator bound with `.model("a")`: after one `Chat::send`, every `UsageRecorded` the session
  delivered has `upstream_model == Some("b")`, and `chat.usage().cost()?` has no `price_unknown`
  line and a `complete_total`.
- No model name, unpriced: the same document and the 12/34 generator bound with no `.model(..)`
  (a value binding has no model name, story:sdk-stack) give, after one `Chat::send`, only
  `price_unknown` lines, a total with `unknown_lines > 0` and no `complete_total`
  (`crates/inference-facade/src/cost.rs:397-402`), and its `Display` starts with `at least 0 USD, `
  and ends with ` unknown`. Bound with
  `.model("gpt-other")`, a name with no entry, the result is the same.
- One adapter unpriced: the same document, and a stack binding `llm` as in the priced case and
  `stt` to the testkit recogniser with `.model("mock-stt")`, a name with no entry, whose records
  carry audio milliseconds and one request (`crates/inference-testkit/src/stt.rs:19`). Over that
  one stack, after one `Transcriber::transcribe_file` and one `Chat::send`,
  `transcriber.usage().cost()?` has only `price_unknown` lines, its total has `unknown_lines > 0`
  and no `complete_total`, and its `Display` contains `unknown`; while `chat.usage().cost()?` has
  no `price_unknown` line and a `complete_total` of 0.000864 USD.
- An unknown count displays as `unknown`, never `0`: after a `Chat::send` whose generator reports no
  usage (`MockGenerator::reporting([None])`), `chat.usage().to_string()` starts with
  `unknown input tokens, unknown output tokens`, and contains no `0 input tokens` and no
  `0 output tokens`.

In `crates/inference-facade/tests/cost.rs`, passing in
`cargo test -p inference-facade --features cost --test cost`: every test that passed before the
lift passes with the lifted `Serving`, and `rg -n 'struct (Serving|Naming)' crates/inference-facade/tests/cost.rs`
exits 1, so the facade holds one mechanism.

And a unit test in `crates/inference/src/usage.rs`, passing in
`cargo test -p inference --features cost --lib usage`, over `Usage::from_records`, with the fixture
read by `include_str!("../tests/fixtures/usage-cost.json")`. The test builds the map itself: one
adapter id, the one the fixture's records carry, to the `PriceBinding` the stack's rule gives for
the model name `gpt-realtime-2.1` (price id, serving model id and binding revision of the entry,
billing `Metered`, charges input, cached input and output text tokens):

- `cost()` of the fixture's records, the fixture's document and that map equals the fixture's
  amount, 0.000864 USD, with no `price_unknown` line.
- `Display`, priced: `to_string()` over the same contains `12 input tokens`, `34 output tokens` and
  `cost: ` followed by the `Display` of that amount, and not `unknown`.
- With an empty map, `cost()` of the same records has only `price_unknown` lines.
- A record with audio tokens gives a `price_unknown` line, never a zero amount.
- `rg -n 'pub fn from_records' crates/inference/src/usage.rs` exits 1 (the constructor is
  `pub(crate)`).


Its diff lists none of `crates/inference/src/lib.rs`, `crates/inference/src/prelude.rs` and
`crates/inference/Cargo.toml` (story:sdk-crate wrote them once, decision D9 of `api-design:sdk`).

## Out of scope

Budgets and spending limits; pricing audio tokens (Q5, an `llm-cost` change).
