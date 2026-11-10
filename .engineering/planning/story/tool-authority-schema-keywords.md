---
format: aep.planning-md/3
id: story:tool-authority-schema-keywords
kind: story
status: active
title: The tool authority admits or the tool attribute refuses every argument type schemars describes
tags:
- from-finding
relations:
- informed_by: review-result:adversary-w25-sdk-tool-macro-pass-1
- decomposes: epic:defects-2026-11-10
- serves: vision:inference
scope:
- confidence: cited
  path: crates/inference-pipeline/src/nodes/tool_authority.rs
- confidence: inferred
  path: crates/inference/tests/tool_macro_schema_keywords.rs
- confidence: inferred
  path: docs/design/sdk.md
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-10T00:17:28Z", actor: "human:timo", revision: 5}
- {from: "proposed", to: "active", at: "2026-11-10T00:17:28Z", actor: "human:timo", revision: 6}
---
## Context

The tool authority validates a tool's argument schema against a subset of JSON Schema
(`crates/inference-pipeline/src/nodes/tool_authority.rs`, `supported` and `admits`), and refuses a
tool whose schema uses any other keyword. The `#[inference::tool]` attribute generates schemas with
schemars; after the second SDK wave the subset admits what schemars emits for strings, booleans,
integers, floats, `Option` of a scalar, `Vec` and structs (subschemas inlined, `format` an
annotation, `minimum`/`maximum` validated). Argument types whose schemars output uses other keywords
are still refused when the authority prepares the tool, with the keyword named: `char`
(`minLength`/`maxLength`), tuples and fixed arrays (`prefixItems`, `minItems`, `maxItems`), enums
with data (`oneOf`), and `Option` of a struct (`anyOf`). Found by the correction round of
story:sdk-tool-macro.

## Planned scope

- `crates/inference-pipeline/src/nodes/tool_authority.rs`: validate `minLength`, `maxLength`,
  `minItems`, `maxItems`, `prefixItems`, `oneOf` and `anyOf`, or decide per keyword that the macro
  refuses the argument type at compile time instead.

## Acceptance

- A macro tool with an argument of each listed type prepares under the authority, or fails to
  compile with a message naming the type; a test per type.

## Scope

Rewritten 2026-11-10 from the merged change (`git diff ff0ba010 9ea2127d`, 4 files).

- **Source:** `crates/inference-pipeline/src/nodes/tool_authority.rs`: `minLength`, `maxLength`, `prefixItems`, `minItems`, `maxItems`, `oneOf`, `anyOf` and `const` validated, lengths read as JSON Schema integers and saturated beyond `u64`, an empty `prefixItems` refused
- **Tests:** `crates/inference/tests/tool_macro_schema_keywords.rs` (1 passed, 6 failed on the base), the adversary files `adversary_u4_schema_keywords.rs` and `adversary_u4_schema_keywords_pass2.rs`
- **Left out:** further keywords (`uniqueItems`, `not`, `allOf`, `exclusiveMaximum`, `pattern`), pre-existing, filed as story:tool-authority-more-schema-keywords
- **Reviews:** review-result:adversary-u4-schema-keywords-pass-1, review-result:adversary-u4-schema-keywords-pass-2
