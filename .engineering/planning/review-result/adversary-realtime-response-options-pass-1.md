---
format: aep.planning-md/3
id: review-result:adversary-realtime-response-options-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on realtime response options: 2 doc-comment findings'
relations:
- reviews: story:realtime-response-options
revision: 1
---
## Report

Adversary pass 1 on `impl/realtime-response-options` (diff `71d6386..4f366bb`), cases committed
on top as `6e0aa5d`: `adapters/inference-openai/tests/adversary15_u4_response_options.rs`, 7
cases. The report as it returned, header and findings verbatim, prose condensed:

```
unit: U4 (story:realtime-response-options), adversary pass
verdict: green
cases: executed 0→7, red 0
origin: introduced 1, pre-existing 1, undecided 0
needs-coordinator: no
```

- Runs: adversary15_u4_response_options 7 passed; fmt and clippy exit 0.
- Cases, kept as regression cases: each option alone sends only its key under `response`
  (including `"input": []` and empty instructions); the bare event is the 0.9.0 text byte for
  byte; hostile text is escaped and adds no top-level key; the same options give the same event;
  an out-of-band response interleaved with the default one arrives by `response_id`; a refused
  `response.create` names its `event_id`; the public types carry the designed derives.
- Callers: `duplex.rs:1079,1086` and `live_duplex.rs:991` call `create_response()`, unchanged.
- Findings: two doc comments.

The first findings block used a vocabulary the store refuses (the coordinator's template was
wrong); the author's corrected block, as it returned:

```findings
[{"file": "adapters/inference-openai/src/client.rs", "line": 609, "category": "contract-drift", "severity": "note", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "ResponseOptions doc says a None option means 'the session's value applies'; input and conversation are not session settings (unset means the default conversation)"}, {"file": "adapters/inference-openai/src/config.rs", "line": 147, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "pre-existing", "message": "ToolChoice doc names only session.update's tool_choice; it is now also written to response.create's response.tool_choice"}]
```
