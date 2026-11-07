---
format: aep.planning-md/3
id: review-result:adversary-w25-sdk-tool-macro-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the tool and main attributes: the authority refuses the macro''s schemas'
relations:
- reviews: story:sdk-tool-macro
revision: 1
---
unit: story:sdk-tool-macro at 28069a8 (base d4728b5), adversary tests fbccabe
verdict: NEEDS-CHANGE
cases: executed 7→16, red 8
origin: introduced 4 / pre-existing 0

1. Blocker, introduced: the tool authority refused the macro's schemas for integer, unsigned, float and struct arguments (`format`, `minimum`/`maximum`, `$defs`; `crates/inference-pipeline/src/nodes/tool_authority.rs:911-945`), the acceptance's own `add(i64, i64)` among them. Tests `a_macro_tool_with_integer_arguments_prepares_under_the_authority` and its unsigned, float and struct siblings.
2. Warning, introduced: the unit struct a tool became shadowed same-named arguments and local bindings in its module (`expected String, found note`). Tests in `adversary_w25_tool_macro_ui.rs`.
3. Note, introduced: a raw identifier kept `r#` in the tool's name.
4. Note, introduced, no test: the non-Deserialize trybuild snapshot copies serde internals and breaks on a serde_core update.

Held: deadline and cancel checks, malformed arguments, two tools in a module, `Option` arguments, hygiene through `::inference::`, the `?` and exit path of `main`.
Remark: `main` does not check the parameter types it is given.
