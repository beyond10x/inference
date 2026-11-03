---
format: aep.planning-md/3
id: review-result:adversary-realtime-usage-cached-audio-input-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: usage reports cached audio input tokens'
relations:
- reviews: story:realtime-usage-cached-audio-input
revision: 1
---
## Adversary pass 1 on story:realtime-usage-cached-audio-input (U4, head 14a28d3)

The adversary's report, as returned (header and findings; prose condensed to the measured parts):

```
unit: story:realtime-usage-cached-audio-input, U4, head 14a28d3 plus 2 untracked adversary test files (working tree)
verdict: CONFIRMED (2 red cases, both outside the named acceptance scenarios; no acceptance scenario broke)
cases: executed 168→171, red 2
origin: introduced 1 / pre-existing 0 / undecided 1
wrote-outside-worktree: scratch only
needs-coordinator: yes
```

- `a_realtime_record_whose_cached_input_is_mostly_audio_is_priced_not_refused` (facade, feature
  `cost`): RED, `price` returns `Cost(InvalidUsage)` for input text 5, cached 16 of which 12 audio.
- `the_usage_concept_page_names_the_cached_input_audio_kind` (core): RED on the unit branch; the
  page was updated on the integration branch by the coordinator (e6f8119).
- `usage_total_kinds_come_out_in_the_order_of_all_with_cached_input_audio` (core): GREEN.
- Suite: `cargo test -p inference-core -p inference-facade --features inference-facade/cost
  --no-fail-fast`, 169 passed, 2 failed.

```findings
[
  {"file": "crates/inference-facade/src/cost.rs", "line": 345, "category": "acceptance", "severity": "warning", "verdict": "CONFIRMED", "origin": "undecided", "message": "Pricing a Realtime record whose cached input (text plus audio) exceeds its input text tokens fails with Cost(InvalidUsage), even when the record carries the cached audio share that would make it consistent (adversary16_u4_cached_audio.rs:89)."},
  {"file": "website/docs/concepts/usage.md", "line": 25, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "The consumer-facing table of quantity kinds and the subset sentence on line 28 do not name cached_input_audio_tokens."},
  {"file": "crates/inference-duplex/src/ledger.rs", "line": 419, "category": "judgement", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "RESPONSE_TOKENS forces an unknown cached_input_audio_tokens into every binding's response record, including the out-of-scope GPT-Live, so a total over records from a provider that does not split cached input by modality is never complete; no current binding loses completeness from it."}
]
```
