---
format: aep.planning-md/3
id: review-result:adversary-w25-sdk-tool-macro-pass-2
kind: review-result
status: active
title: 'Adversary pass 2 on the tool attribute: a tool function converts into no pair'
relations:
- reviews: story:sdk-tool-macro
revision: 1
---
unit: story:sdk-tool-macro at 6a98e93 (correction round 1 on fbccabe), adversary tests db72eff
verdict: NEEDS-CHANGE
cases: executed 2→5, red 3
origin: introduced 1 / pre-existing 1 (plus one introduced doc note)

1. Warning, introduced: the tool is now a function, which converts into no `(Tool, Box<dyn ToolExecutor>)` pair, so `.tool(local_time)` in `docs/design/sdk.md` and story:sdk-agent's planned `tool(impl Into<..>)` break. Test `adversary_w25_tool_macro_pass2_ui::a_tool_converts_into_the_pair_agent_tool_takes`.
2. Note, introduced: a self-referential argument still yields `$defs`/`$ref` (schemars inlines only types not being generated), contradicting the macro doc; the authority refuses it cleanly.
3. Note, pre-existing: the tool authority accepted any value for an annotation keyword (`format: 5` was prepared). Test `a_format_that_is_not_a_string_fails_preparation`.

Fixed in 819e603: `IntoTool` in `inference-tools`, implemented for the pair and for any `FnOnce()` returning it; the macro doc says a recursive argument keeps `$defs`/`$ref` and fails preparation; annotations must take JSON Schema's form (strings for `title`, `description`, `format`, `$schema`, `$id`, `$comment`; an array for `examples`).
