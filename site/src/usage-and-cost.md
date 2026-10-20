# Usage and cost

The goal: every model call in a run, STT, LLM, TTS or native speech, hosted or local, emits one
usage record on the run's reliable event path. A caller that supplies a price document gets a cost
per record and per session. Part of that exists now.

## What exists

| What | Status |
|---|---|
| Native speech sessions record the provider's response and transcription usage (`inference-duplex`). | <span class="st st-rel">0.5.0</span> |
| The usage vocabulary, `inference.usage`, in `inference-core`: one `UsageRecord` per model call and its `UsageRecorded` event. | <span class="st st-rel">0.6.0</span> |
| `realtime-voice` prints usage per response and session totals, and estimates cost from a price table you supply. | <span class="st st-rel">0.6.0</span> |

A `UsageRecord` names who made the call (adapter, model, run, node, session, speech session,
response where there is one), when (start and end on the wall clock), whether the provider marked
its counts final, and a list of quantities. Each quantity has a kind and a source:

| Kinds | Sources |
|---|---|
| input and output text tokens, cached input tokens, cache-creation input tokens, input and output audio tokens, input and output audio milliseconds, input characters, requests, local compute milliseconds | `provider_reported`, `measured`, `unknown` |

**Unknown is never zero.** A quantity's value is absent exactly when its source is `unknown`; a
record whose value and source disagree is refused (`QuantityMismatch`).

## Cost estimates in the example

```sh
cargo run -p realtime-voice --example talk -- --provider openai --prices prices.json
```

No prices are built in. The table is yours, per million tokens:

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

Replace each `"<price>"` with a JSON number, at least zero, with at most six decimals; `cached_in`
is optional and unknown fields are refused. A count the provider did not report prints as `?`, and
no cost is estimated from a count that is not known.

## Planned

| What | Status |
|---|---|
| LLM usage: one record per turn, from the `llm` project's usage | <span class="st st-plan">planned</span> |
| Native speech usage as usage records (OpenAI Realtime, Nova Sonic) | <span class="st st-plan">planned</span> |
| STT usage: input audio and local compute per recognition | <span class="st st-plan">planned</span> |
| TTS usage: characters, output audio and compute per synthesis | <span class="st st-plan">planned</span> |
| Run and session totals in the facade, kept by the replay recorder, never dropped with telemetry | <span class="st st-plan">planned</span> |
| Cost records from a caller price document, behind a facade `cost` feature | <span class="st st-plan">planned</span> |
| Picking the voice provider by configuration and comparing latency and usage across providers | <span class="st st-plan">planned</span> |
