---
format: aep.planning-md/3
id: review-result:adversary-realtime-event-reads-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the realtime event reads: 3 introduced findings on nameless calls'
relations:
- reviews: story:realtime-committed-previous-item
- reviews: story:realtime-function-call-name-optional
revision: 1
---
## Report

Adversary pass 1 on `impl/realtime-event-reads` (diff `6cd4f0f..3404c45`), cases committed on
top as `a63f399`: `adapters/inference-openai/tests/adversary15_u3_event_reads.rs` and
`adapters/inference-openai/tests/adversary15_u3_duplex.rs` (feature `duplex`). The report as it
returned, header and findings verbatim, prose condensed:

```
unit: U3
verdict: red
cases: executed 0→9, red 3
origin: introduced 3, pre-existing 0, undecided 0
needs-coordinator: yes
```

- Runs: adversary15_u3_event_reads 5 passed, 2 failed; adversary15_u3_duplex 1 passed, 1
  failed; event_reads 10 passed. fmt and clippy clean.
- Empty name matching an empty-named tool, confirmed: `Tool::declare` does not check names; a
  call with no name reaches `duplex.rs:894` (`name.as_deref().unwrap_or_default()`) and is
  proposed as that tool. Red: `duplex_a_nameless_call_is_not_offered_when_a_tool_is_named_empty`.
  Only a caller who declares an empty-named tool reaches it.
- A name remembered again after its call was filled, confirmed: `response.output_item.done`
  arrives after `response.function_call_arguments.done`, so `client.rs:82` stores the entry again
  and it stays until the 64-entry cap evicts it; the docs at `client.rs:47` and `:86` say it is
  forgotten; the unit's `a_carried_name_fills_one_call_only` omits `output_item.done`. Red:
  `an_output_item_done_after_its_call_is_not_remembered_again`.
- Suspected: a call carrying `"name": ""` keeps `Some("")` and drops the carried name
  (`client.rs:92`, `event.rs:220`). Red: `an_empty_name_on_the_call_is_filled_from_the_output_item`.
  Whether the server sends an empty name is unknown.
- Kept green as regression cases: hostile output items (10 shapes) do not panic and name
  nothing; the fill matches on `item_id`; a later output item renames; a malformed call does not
  use up the name; an empty `previous_item_id` is `Some("")`; through duplex a carried name
  proposes the offered tool end to end.

The first findings block used a vocabulary the store refuses (the coordinator's template was
wrong); the author's corrected block, as it returned:

```findings
[{"file": "adapters/inference-openai/src/duplex.rs", "line": 894, "category": "boundary", "severity": "note", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "a call with no name is looked up as tool \"\" and proposed as a declared empty-named tool instead of refused ToolNotOffered (duplex_a_nameless_call_is_not_offered_when_a_tool_is_named_empty red)"}, {"file": "adapters/inference-openai/src/client.rs", "line": 82, "category": "contract-drift", "severity": "note", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "output_item.done after its call re-remembers the filled entry, contrary to the docs at client.rs:47 and :86; a_carried_name_fills_one_call_only omits output_item.done (an_output_item_done_after_its_call_is_not_remembered_again red)"}, {"file": "adapters/inference-openai/src/client.rs", "line": 92, "category": "judgement", "severity": "note", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "a call sent with name \"\" keeps Some(\"\") and drops the name carried from its output item (an_empty_name_on_the_call_is_filled_from_the_output_item red)"}]
```
